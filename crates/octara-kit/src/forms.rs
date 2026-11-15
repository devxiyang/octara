//! Labeled controls and finite, externally controlled radio choices.

use crate::Model;
use octara::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

/// The helper text and current validation of a [`field`]. Compute it from ordinary application
/// state; an absent or blank error means the control is valid.
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

/// A label, retained `control`, helper text and current validation.
///
/// `id` names the actual control. The wrapper is `{id}-field`, its visible label `{id}-label`,
/// helper `{id}-help`, and error `{id}-error`. Pressing the label focuses the control. Its
/// accessible name is `label`; its description combines the helper and any nonblank error.
/// `required` and `invalid` are applied to the control itself. The error is a polite live region.
///
/// `info` is tracked and evaluated coherently through a memo. Changes only update properties
/// and supporting text: the control keeps its identity, editor, selection, undo and input method.
/// The control's appearance, model and handlers remain the caller's.
/// Pass the actual focusable control, not a container around it. Set `disabled` and handlers
/// on that control before passing it in; chaining them on the returned field affects only its
/// wrapper. For example, `field("name", "Name", text_field(name).disabled(busy), info)`.
#[component]
#[track_caller]
pub fn field(id: &str, label: impl Into<String>, control: El, info: impl Fn() -> FieldInfo + 'static) -> El {
    let label = label.into();
    let info = memo(info);
    let focus_id = id.to_owned();
    let help_id = format!("{id}-help");
    let error_id = format!("{id}-error");
    col()
        .id(format!("{id}-field"))
        .w_full()
        .min_w(0.0)
        .gap_1p5()
        .child(
            text(label.clone())
                .id(format!("{id}-label"))
                .text_sm()
                .font_medium()
                .text_color(Token::Text)
                .min_h(24.0)
                .when_touch(|p| p.min_h(crate::TOUCH_TARGET))
                .select_none()
                .on_press(move || focus(&focus_id)),
        )
        .child(
            control
                .id(id.to_owned())
                .label(label)
                .description(move || info.get().description())
                .required(move || info.get().required)
                .invalid(move || info.get().error().is_some()),
        )
        .child(show(
            move || !info.get().description.trim().is_empty(),
            move || text(move || info.get().description).id(help_id.clone()).text_sm().text_color(Token::TextMuted).into(),
        ))
        .child(show(
            move || info.get().error().is_some(),
            move || {
                text(move || info.get().error().unwrap_or_default().to_owned())
                    .id(error_id.clone())
                    .role(Role::Alert)
                    .live(Live::Polite)
                    .text_sm()
                    .text_color(Token::Danger)
                    .into()
            },
        ))
}

/// One choice in a [`radio_group`]. IDs must be unique and remain stable when options move or
/// their labels change. The selected model stores the ID, so identical labels remain distinct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub disabled: bool,
}

impl RadioOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), description: String::new(), disabled: false }
    }

    /// Supporting text shown beneath the label and exposed as the radio's description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Whether this choice is unavailable. It remains visible and keyboard navigation skips it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Clone, PartialEq)]
struct RadioRow {
    option: RadioOption,
    order: usize,
}

/// A labeled group choosing one option ID in the caller's `value`.
///
/// The group is `id`, its visible label `{id}-label`, and each radio
/// `{id}-option-{option.id}`. `options` is tracked; insertion, removal, reordering, renaming and
/// availability update retained rows. Removing the chosen option leaves the caller's value
/// intact, with no radio checked. This is a retained list for finite groups.
///
/// From outside the group, Tab enters at the checked enabled radio, otherwise the remembered
/// enabled radio, otherwise the first enabled radio. The next Tab leaves the group.
/// Focus alone does not choose. Arrow keys wrap between enabled options and choose them;
/// Home and End choose the first and last enabled options.
/// Press, Space and Enter choose the focused option. Disabled options refuse activation.
#[component]
#[track_caller]
pub fn radio_group(
    id: &str,
    label: impl Into<String>,
    value: impl Model<Option<String>>,
    options: impl Fn() -> Vec<RadioOption> + 'static,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    let value: Rc<dyn Model<Option<String>>> = Rc::new(value);
    component_keyed("RadioGroup", id.clone(), move || {
        let options = memo(options);
        let rows = state_list("options", Vec::<RadioRow>::new, |row: &RadioRow| row.option.id.clone());
        let ordered = rows.sort_by(|row| row.order);
        watch(
            move || options.get(),
            move |options, _| {
                let keys: HashSet<_> = options.iter().map(|option| option.id.as_str()).collect();
                rows.retain(|row| keys.contains(row.option.id.as_str()));
                let mut seen = HashSet::new();
                for (order, option) in options.iter().enumerate() {
                    let row = RadioRow { option: option.clone(), order };
                    if !seen.insert(option.id.clone()) {
                        // Keep the ordinary list diagnostic for duplicate stable IDs.
                        rows.push(row);
                    } else if let Some(old) = rows.item(&option.id) {
                        if untrack(|| old.get()) != row {
                            rows.update(&option.id, |old| *old = row);
                        }
                    } else {
                        rows.push(row);
                    }
                }
            },
        );
        let option_id = id.clone();
        col()
            .id(id.clone())
            .role(Role::RadioGroup)
            .label(label.clone())
            .focus_group()
            .w_full()
            .min_w(0.0)
            .gap_1()
            .child(text(label).id(format!("{id}-label")).text_sm().font_medium().text_color(Token::Text))
            .child(each(
                move || ordered.read_only(),
                move |key, item| {
                    let (checked, shown, dot, pick) = (value.clone(), value.clone(), value.clone(), value.clone());
                    let (checked_key, shown_key, dot_key, pick_key) = (key.clone(), key.clone(), key.clone(), key.clone());
                    let mark = el()
                        .size_4()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border()
                        .border_color(Token::Border)
                        .bg(Token::Background)
                        .color_motion(Spring::SNAP)
                        .checked(move || shown.get().as_ref() == Some(&shown_key))
                        .style_when(State::CHECKED, |p| p.bg(Token::Accent).border_color(Token::Accent))
                        .child(
                            el().size(6.0)
                                .rounded_full()
                                .bg(Token::AccentText)
                                .opacity(move || if dot.get().as_ref() == Some(&dot_key) { 1.0 } else { 0.0 }),
                        )
                        .a11y_hidden();
                    row()
                        .id(format!("{option_id}-option-{key}"))
                        .role(Role::RadioButton)
                        .label(move || item.with(|row| row.option.label.clone()))
                        .description(move || item.with(|row| row.option.description.clone()))
                        .checked(move || checked.get().as_ref() == Some(&checked_key))
                        .disabled(move || item.with(|row| row.option.disabled))
                        .focusable()
                        .w_full()
                        .min_w(0.0)
                        .min_h(32.0)
                        .when_touch(|p| p.min_h(crate::TOUCH_TARGET))
                        .items_center()
                        .gap_3()
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .border()
                        .border_color(Paint::Color(Color::TRANSPARENT))
                        .text_sm()
                        .text_color(Token::Text)
                        .color_motion(Spring::SNAP)
                        .hover(|p| p.bg(Token::SurfaceHover))
                        .active(|p| p.bg(Token::SurfacePressed))
                        .focus_visible(|p| p.border_color(Token::Focus))
                        .style_when(State::DISABLED, |p| p.opacity(0.5))
                        .child(mark)
                        .child(
                            col()
                                .flex_1()
                                .min_w(0.0)
                                .gap_0p5()
                                .child(text(move || item.with(|row| row.option.label.clone())).select_none())
                                .child(show(
                                    move || item.with(|row| !row.option.description.trim().is_empty()),
                                    move || {
                                        text(move || item.with(|row| row.option.description.clone()))
                                            .text_color(Token::TextMuted)
                                            .select_none()
                                            .into()
                                    },
                                )),
                        )
                        .on_press(move || pick.set(Some(pick_key.clone())))
                        .into()
                },
            ))
            .into()
    })
}
