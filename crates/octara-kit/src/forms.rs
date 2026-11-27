//! Labeled controls and finite, externally controlled radio choices.

use crate::{controls::TOUCH_TARGET, row_value};
use octara::prelude::*;
use octara_headless::{Choice, FieldState, FieldsetState, RadioGroupState, field as headless};
use std::{fmt::Display as KeyDisplay, hash::Hash};

/// The helper text and application validation of a [`field`]. An absent or blank error leaves
/// validation to the control.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldInfo {
    pub description: String,
    pub error: Option<String>,
    pub required: bool,
}

impl FieldInfo {
    fn error(&self) -> Option<&str> {
        self.error.as_deref().filter(|error| !error.trim().is_empty())
    }

    fn description(&self) -> String {
        match (self.description.trim().is_empty(), self.error()) {
            (false, Some(error)) => format!("{}\n{error}", self.description),
            (true, Some(error)) => error.to_owned(),
            (false, None) => self.description.clone(),
            (true, None) => String::new(),
        }
    }
}

/// Wraps an existing control with a label, description and current validation.
/// `id` identifies the control; the wrapper is `{id}-field`, the label `{id}-label`, the
/// description text `{id}-help` and the error `{id}-error`. Activating the label focuses the
/// control. Label and validation changes preserve the control's mounted identity and editor state.
#[ui]
#[track_caller]
pub fn field<C>(build: &mut BuildCx<'_>, id: &str, label: impl IntoProp<String>, control: El<C>, info: impl IntoRead<FieldInfo>) -> El
where
    El<C>: IntoView,
{
    let state = FieldState::new(build, false);
    let info = info.into_read();
    let focus_id = id.to_owned();
    let help_id = format!("{id}-help");
    let error_id = format!("{id}-error");
    let label = state
        .label(text(label.into_prop()))
        .test_id(format!("{id}-label"))
        .text_size(FontSizeToken::Sm)
        .text_weight(500)
        .text_color(ColorToken::Text)
        .min_h(24.0)
        .when(When::Touch, |p| p.min_h(TOUCH_TARGET))
        .select_none()
        .preserve_focus_on_press()
        .on_press(move |w| w.focus(&focus_id));
    let control = state
        .control(control.test_id(id.to_owned()))
        .description(move |r: &mut ReadCx<'_>| r.with(info, FieldInfo::description))
        .required(move |r: &mut ReadCx<'_>| r.with(info, |info| info.required))
        .invalid_when(move |r| r.with(info, |info| info.error().is_some()))
        .into_view();
    let support = show(
        move |r| r.with(info, |info| !info.description.trim().is_empty() || info.error().is_some()),
        move |_| {
            let help_id = help_id.clone();
            let error_id = error_id.clone();
            state.description(
                col()
                    .gap(sp(1.5))
                    .child(show(
                        move |r| r.with(info, |info| !info.description.trim().is_empty()),
                        move |_| {
                            text(move |r: &mut ReadCx<'_>| r.with(info, |info| info.description.clone()))
                                .test_id(help_id.clone())
                                .text_size(FontSizeToken::Sm)
                                .text_color(ColorToken::TextMuted)
                        },
                    ))
                    .child(show(
                        move |r| r.with(info, |info| info.error().is_some()),
                        move |_| {
                            text(move |r: &mut ReadCx<'_>| r.with(info, |info| info.error().unwrap_or_default().to_owned()))
                                .test_id(error_id.clone())
                                .role(Role::Alert)
                                .live(Live::Polite)
                                .text_size(FontSizeToken::Sm)
                                .text_color(ColorToken::Danger)
                        },
                    )),
            )
        },
    );
    let frame = col().test_id(format!("{id}-field")).w(rel(1.0)).min_w(0.0).gap(sp(1.5));
    headless::field(frame, state, move |_| View::fragment([label.into_view(), control, support]))
}

/// Groups fields under an accessible legend and lets them share a tracked disabled condition.
/// For a standalone element inside the factory, `headless::field::control(build, element)`
/// applies the same inherited state.
#[ui]
#[track_caller]
pub fn fieldset<V: IntoView + 'static>(
    build: &mut BuildCx<'_>,
    id: &str,
    legend: impl IntoText,
    disabled: impl IntoProp<bool>,
    content: impl FnOnce(&mut BuildCx<'_>) -> V + 'static,
) -> El {
    let state = FieldsetState::new(build, disabled);
    let legend = state.legend(text(legend)).test_id(format!("{id}-legend")).text_weight(600).text_color(ColorToken::Text);
    let frame = col()
        .test_id(id.to_owned())
        .w(rel(1.0))
        .min_w(0.0)
        .gap(sp(3.0))
        .p(sp(4.0))
        .rounded(RadiusToken::Lg)
        .border(1.0)
        .border_color(ColorToken::Border)
        .child(legend);
    headless::fieldset(frame, state, content)
}

/// A labeled group choosing one option ID in the caller's `value`.
///
/// The group is `id`, its visible label `{id}-label`, and each radio
/// `{id}-option-{key}`. `options` is tracked; insertion, removal, reordering, renaming and
/// availability update retained rows. Removing the chosen option leaves the caller's value
/// intact, with no radio checked. This is a retained list for finite groups.
/// `id` also keys the owner namespace for row work registered by model setters; named row
/// state follows this component when sibling groups change order across hot reloads.
///
/// From outside the group, Tab enters at the checked enabled radio, otherwise the remembered
/// enabled radio, otherwise the first enabled radio. The next Tab leaves the group.
/// Focus alone does not choose. Arrow keys wrap between enabled options and choose them;
/// Home and End choose the first and last enabled options.
/// Press, Space and Enter choose the focused option. Disabled options refuse activation.
#[ui]
#[track_caller]
pub fn radio_group<K: Clone + Eq + Hash + KeyDisplay + 'static>(
    id: &str,
    label: impl Into<String>,
    value: Binding<Option<K>>,
    options: ReadList<K, Choice>,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    scope(format!("RadioGroup[{id}]"), move |b| -> View {
        let state = RadioGroupState::new(b, options, value);
        let label_id = format!("{id}-label");
        state
            .group(col())
            .test_id(id.clone())
            .labelled_by(label_id.clone())
            .w(rel(1.0))
            .min_w(0.0)
            .gap(sp(1.0))
            .child(text(label).test_id(label_id).text_size(FontSizeToken::Sm).text_weight(500).text_color(ColorToken::Text))
            .child(each(options, move |b, key, item| {
                let option_id = format!("{id}-option-{key}");
                let selected_key = key.clone();
                let selected = b.memo(move |r| value.with(r, |selected| selected.as_ref() == Some(&selected_key)));
                let description = item.clone();
                let label = item.clone();
                let described = item.clone();
                let mark = el()
                    .size(sp(4.0))
                    .shrink(0.0)
                    .align(AlignItems::Center)
                    .justify(JustifyContent::Center)
                    .rounded(Radius::Full)
                    .border(1.0)
                    .border_color(ColorToken::Border)
                    .bg(ColorToken::Background)
                    .style(StyleProp::ColorMotion(Spring::SNAP.into()))
                    .checked(selected)
                    .when(State::CHECKED, |p| p.bg(ColorToken::Accent).border_color(ColorToken::Accent))
                    .child(
                        el().size(6.0)
                            .rounded(Radius::Full)
                            .bg(ColorToken::AccentText)
                            .opacity(move |r: &mut ReadCx<'_>| if r.get(selected) { 1.0_f32 } else { 0.0 }),
                    )
                    .a11y_hidden();
                state
                    .radio(row(), key.clone())
                    .test_id(option_id)
                    .description(move |r: &mut ReadCx<'_>| row_value(&description, r, |row| row.description.clone()))
                    .w(rel(1.0))
                    .min_w(0.0)
                    .min_h(32.0)
                    .when(When::Touch, |p| p.min_h(TOUCH_TARGET))
                    .align(AlignItems::Center)
                    .gap(sp(3.0))
                    .px(sp(2.0))
                    .py(sp(1.5))
                    .rounded(RadiusToken::Md)
                    .border(1.0)
                    .border_color(ColorValue::Color(Color::TRANSPARENT))
                    .text_size(FontSizeToken::Sm)
                    .text_color(ColorToken::Text)
                    .style(StyleProp::ColorMotion(Spring::SNAP.into()))
                    .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
                    .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed))
                    .when(State::FOCUS_VISIBLE, |p| p.border_color(ColorToken::Focus))
                    .when(State::DISABLED, |p| p.opacity(0.5))
                    .child(mark)
                    .child(
                        col()
                            .grow(1.0)
                            .basis(0.0)
                            .min_w(0.0)
                            .gap(sp(0.5))
                            .child(text(move |r: &mut ReadCx<'_>| row_value(&label, r, |row| row.label.clone())).select_none())
                            .child(show(
                                move |r| row_value(&described, r, |row| !row.description.trim().is_empty()),
                                move |_| {
                                    let item = item.clone();
                                    text(move |r: &mut ReadCx<'_>| row_value(&item, r, |row| row.description.clone()))
                                        .text_color(ColorToken::TextMuted)
                                        .select_none()
                                },
                            )),
                    )
            }))
            .into()
    })
}
