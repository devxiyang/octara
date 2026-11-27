//! Themed controls with direct focus and pointer behavior on their semantic nodes.

use crate::{
    KitLocale,
    parts::{button_part, button_root, editor_content, editor_style, focus_outline, input_group_outline},
};
use octara::prelude::*;
use octara_headless::{checkbox::CheckValue, interactive};
use octara_icons as icons;

pub(crate) const TOUCH_TARGET: f32 = 44.0;

/// The visual size of a Kit control. Touch targets are at least 44 by 44.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Sm,
    #[default]
    Md,
    Lg,
}

/// The emphasis and purpose of a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Destructive,
}

/// Semantic colors shared by badges and callouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

/// A themed button with the specified size and emphasis.
#[ui]
#[track_caller]
pub fn button(label: impl IntoText, size: Size, variant: ButtonVariant) -> El {
    styled_button(text(label), size, variant)
}

#[inline(never)]
#[track_caller]
fn styled_button(label: El, size: Size, variant: ButtonVariant) -> El {
    button_part(row().select_none().child(label.select_none().min_w(0)), size, variant).text_align(TextAlign::Center)
}

/// A button that toggles a controlled boolean and exposes its pressed state.
#[ui]
#[track_caller]
pub fn toggle_button(label: impl IntoText, pressed: Binding<bool>, size: Size, variant: ButtonVariant) -> El {
    styled_toggle_button(button(label, size, variant), pressed)
}

#[inline(never)]
fn styled_toggle_button(button: El, pressed: Binding<bool>) -> El {
    interactive::toggle_button(button, pressed).when(State::CHECKED, |style| style.border_color(ColorToken::Accent))
}

/// An icon-only button with an accessible name and matching tooltip.
#[ui]
#[track_caller]
pub fn icon_button(glyph: impl Into<Icon>, label: impl Into<String>) -> El {
    let label = label.into();
    styled_icon_button(glyph.into(), label)
}

#[inline(never)]
#[track_caller]
fn styled_icon_button(glyph: Icon, label: String) -> El {
    icon_target().child(icon(glyph).a11y_hidden()).label(label.clone()).tooltip(label)
}

fn icon_target() -> El {
    button_root(el())
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .size(28.0)
        .shrink(0.0)
        .when(When::Touch, |p| p.min_w(TOUCH_TARGET).min_h(TOUCH_TARGET))
        .rounded(RadiusToken::Md)
        .border_color(Color::TRANSPARENT)
        .text_color(ColorToken::TextMuted)
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover).text_color(ColorToken::Text))
        .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed))
}

fn toggle_target(visual: El) -> El {
    focus_outline(el())
        .child(visual.a11y_hidden())
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .border_color(Color::TRANSPARENT)
        .rounded(RadiusToken::Md)
        .when(When::Touch, |p| p.min_w(TOUCH_TARGET).min_h(TOUCH_TARGET))
}

/// A named checkable control bound to a boolean or [`Checked`]. Activating a mixed checkbox
/// checks it. Application `on_toggle` handlers observe the model after the binding is updated.
#[ui]
#[track_caller]
pub fn checkbox<T: CheckValue>(value: Binding<T>) -> El {
    let visual = checkbox_visual(
        el().checked(move |r: &mut ReadCx<'_>| r.get(value).checked() != Checked::Unchecked),
        keyed(
            move |r| match r.get(value).checked() {
                Checked::Unchecked => "unchecked",
                Checked::Checked => "checked",
                Checked::Mixed => "mixed",
            },
            checkbox_mark,
        ),
    );
    octara_headless::checkbox::checkbox(visual, value)
}

#[inline(never)]
fn checkbox_visual(square: El, mark: View) -> El {
    toggle_target(
        square
            .size(sp(4.0))
            .align(AlignItems::Center)
            .justify(JustifyContent::Center)
            .rounded(RadiusToken::Sm)
            .border(1.0)
            .border_color(ColorToken::Border)
            .bg(ColorToken::Background)
            .when(State::CHECKED, |p| p.bg(ColorToken::Accent).border_color(ColorToken::Accent))
            .child(mark),
    )
}

fn checkbox_mark(_: &mut BuildCx<'_>, mark: &&str) -> Option<El> {
    let glyph = match *mark {
        "checked" => icons::CHECK,
        "mixed" => icons::MINUS,
        _ => return None,
    };
    Some(icon(glyph.into()).size(12.0).text_color(ColorToken::AccentText))
}

/// An on/off switch. Application `on_toggle` handlers observe the updated binding.
#[ui]
#[track_caller]
pub fn switch(on: Binding<bool>) -> El {
    let thumb = el().size(sp(4.0)).rounded(Radius::Full).bg(ColorToken::Background);
    let track = el()
        .child(thumb)
        .w(sp(8.0))
        .h(sp(5.0))
        .p(2.0)
        .align(AlignItems::Center)
        .rounded(Radius::Full)
        .bg(ColorToken::Border)
        .style(StyleProp::ColorMotion(Spring::SNAP.into()))
        .checked(move |r: &mut ReadCx<'_>| r.get(on))
        .when(State::CHECKED, |p| p.bg(ColorToken::Accent).justify(JustifyContent::End));
    octara_headless::checkbox::switch(toggle_target(track), on)
}

/// A single-line editor. Application `on_input` handlers observe the updated binding.
#[ui]
#[track_caller]
pub fn text_field(value: Binding<String>) -> TextInput {
    text_editor(editor_style(input()), value)
}

/// A multiline editor. Use `.rows(...)` to set its height limits and `.submit_on_enter()`
/// to make Enter submit.
#[ui]
#[track_caller]
pub fn text_area(value: Binding<String>) -> TextInput {
    text_editor(editor_style(textarea()), value)
}

/// A search editor with Enter submission and a clear button when its query is nonempty.
/// `id` names the input and `{id}-clear` names its clear button. `configure` receives the
/// actual editor so the caller can set a placeholder or submit callback.
#[ui]
#[track_caller]
pub fn search_field(
    build: &mut BuildCx<'_>,
    id: &str,
    label: impl Into<String>,
    value: Binding<String>,
    configure: impl FnOnce(TextInput) -> TextInput,
) -> El {
    let locale = KitLocale::get(build);
    let editor = configure(group_editor(label.into(), value).placeholder(locale.search).submit_on_enter());
    search_group(id, value, locale.clear_search, editor)
}

#[inline(never)]
fn search_group(id: &str, value: Binding<String>, clear_label: &'static str, editor: TextInput) -> El {
    let focus_id = id.to_owned();
    let clear_id = format!("{id}-clear");
    let editor = editor.test_id(id.to_owned()).grow(1.0).basis(0.0);
    editor_group()
        .child(icon(icons::SEARCH.into()).ml(sp(1.0)).shrink(0.0).text_color(ColorToken::TextMuted).size(sp(4.0)).a11y_hidden())
        .child(editor)
        .child(show(
            move |r| r.with(value, |query| !query.is_empty()),
            move |_| {
                let focus_id = focus_id.clone();
                icon_button(icons::X, clear_label).test_id(clear_id.clone()).preserve_focus_on_press().on_press(move |w| {
                    w.set(value, String::new());
                    w.focus(&focus_id);
                })
            },
        ))
}

/// A secure editor with a reveal button that toggles masking on the same input.
/// `id` names the input and `{id}-reveal` names its toggle. `configure` receives the
/// actual editor so the caller can set a placeholder or submit callback.
#[ui]
#[track_caller]
pub fn password_field(
    build: &mut BuildCx<'_>,
    id: &str,
    label: impl Into<String>,
    value: Binding<String>,
    configure: impl FnOnce(TextInput) -> TextInput,
) -> El {
    let locale = KitLocale::get(build);
    let revealed = build.signal(false);
    let editor = configure(group_editor(label.into(), value));
    password_group(id, locale, revealed, editor)
}

#[inline(never)]
fn password_group(id: &str, locale: KitLocale, revealed: Signal<bool>, editor: TextInput) -> El {
    let reveal_label = move |r: &mut ReadCx<'_>| if r.get(revealed) { locale.hide_password } else { locale.show_password };
    let editor = editor.test_id(id.to_owned()).secure(move |r: &mut ReadCx<'_>| !r.get(revealed)).grow(1.0).basis(0.0);
    let reveal = icon_target()
        .test_id(format!("{id}-reveal"))
        .label(bind(move |r| reveal_label(r).to_owned()))
        .tooltip(bind(move |r| reveal_label(r).to_owned()))
        .selected(revealed)
        .preserve_focus_on_press()
        .child(keyed(
            move |r| r.get(revealed),
            |_, visible| {
                let glyph = if *visible { icons::EYE_OFF } else { icons::EYE };
                icon(glyph.into()).a11y_hidden()
            },
        ))
        .on_press(move |w| w.update(revealed, |visible| *visible = !*visible));
    editor_group().child(editor).child(reveal)
}

fn editor_group() -> El {
    input_group_outline(row())
        .items_center()
        .min_w(0.0)
        .when(When::Touch, |style| style.min_h(TOUCH_TARGET))
        .px(sp(1.0))
        .rounded(RadiusToken::Md)
        .bg(ColorToken::Background)
}

#[inline(never)]
#[track_caller]
fn group_editor(label: String, value: Binding<String>) -> TextInput {
    text_editor(editor_content(input()).min_w(0.0), value).label(label)
}

fn text_editor(editor: TextInput, value: Binding<String>) -> TextInput {
    editor.value(value).behavior_first(|behavior| behavior.on_input(move |w, text| w.set(value, text.to_owned())))
}

/// Files from the system drop or chooser, or an error opening or reading the chosen file.
#[derive(Debug)]
pub enum FileDrop {
    Dropped(Vec<DroppedFile>),
    Picked(PickedFile),
    Error(ServiceError),
}

/// A drop target that also opens the platform's file chooser by pointer or keyboard.
/// `accept` and `max_bytes` configure the chooser. The application validates dropped files.
/// Cancelling the chooser leaves the application state as it was.
#[ui]
#[track_caller]
pub fn file_dropzone(
    id: &str,
    label: impl Into<String>,
    accept: impl Into<String>,
    max_bytes: usize,
    on_files: impl Fn(&mut WriteCx<'_>, FileDrop) + Clone + 'static,
) -> El {
    let label = label.into();
    let accept = accept.into();
    let on_drop = on_files.clone();
    button_root(col())
        .test_id(id.to_owned())
        .label(label.clone())
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .gap(sp(2.0))
        .p(sp(4.0))
        .w(rel(1.0))
        .min_w(TOUCH_TARGET)
        .min_h(TOUCH_TARGET)
        .rounded(RadiusToken::Lg)
        .bg(ColorToken::Surface)
        .text_size(FontSizeToken::Sm)
        .text_color(ColorToken::Text)
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed))
        .when(State::DRAG_OVER, |p| p.border_color(ColorToken::Accent).bg(ColorToken::SurfaceHover))
        .child(icon(icons::UPLOAD.into()).size(sp(5.0)).a11y_hidden())
        .child(text(label).select_none().a11y_hidden())
        .on_file_drop(move |w, files| on_drop(w, FileDrop::Dropped(files.to_vec())))
        .on_press(move |w| {
            let on_files = on_files.clone();
            w.request_service(ServiceRequest::PickFile { max_bytes, accept: accept.clone() }, move |w, outcome| match outcome {
                ServiceOutcome::FilePicked(Some(file)) => on_files(w, FileDrop::Picked(file)),
                ServiceOutcome::Error(error) if error != ServiceError::Cancelled => on_files(w, FileDrop::Error(error)),
                _ => {}
            });
        })
}
