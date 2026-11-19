//! Controlled tab bars and compact radio choices, composed from retained focus groups.

use crate::{Choice, Model, RadioOption};
use octara::prelude::*;
use std::rc::Rc;

/// A tab bar with manual activation, bound to a stable [`Choice`] ID in `value`.
///
/// `id` names the `TabList`; each `Tab` is `{id}-tab-{option.id}`. `label` is the bar's
/// accessible name. The tracked options retain their targets when reordered, renamed or
/// disabled. A missing or removed selected ID leaves `value` unchanged and no tab selected.
/// `id` also keys the component's owner namespace: work registered by a model setter belongs
/// to its tab row, and named row state follows the component across reordered hot reloads.
///
/// Tab enters at the selected enabled tab, otherwise its remembered enabled tab, otherwise
/// its first enabled tab. The next Tab leaves the bar. Left and Right move focus past disabled
/// tabs and wrap at the ends. Home and End focus the first and last enabled tabs. Press, Enter
/// and Space activate the focused tab.
///
/// Each tab points to the panel with id `{id}-panel-{option.id}` while it is accessible.
/// The application owns its panels. Give each `Role::TabPanel` and label it from its tab.
/// Use `show` to mount a panel on selection or bind its display to retain its editor, undo
/// and scroll state.
///
/// ```rust,ignore
/// col().id("views-panel-notes").role(Role::TabPanel).labelled_by("views-tab-notes")
///     .style_with(move |p| if selected.get().as_deref() == Some("notes") { p.flex() } else { p.hidden() })
///     .child(text_field(draft))
/// ```
/// The application controls panel focus, background work and navigation stacks, including
/// when selection changes externally.
#[ui]
#[track_caller]
pub fn tabs(id: &str, label: impl Into<String>, value: impl Model<Option<String>>, options: impl Fn() -> Vec<Choice> + 'static) -> View {
    let id = id.to_owned();
    let label = label.into();
    let value: Rc<dyn Model<Option<String>>> = Rc::new(value);
    scope(format!("Tabs[{id}]"), move || -> View {
        row()
            .id(id.clone())
            .role(Role::TabList)
            .label(label)
            .focus_group(FocusGroup::horizontal().wrap())
            .w_full()
            .min_w(0.0)
            .flex_wrap()
            .gap_1()
            .child(each(
                move || options().into_iter().map(|option| (option.id.clone(), option)).collect::<Vec<_>>(),
                move |key, item| {
                    let (read, pick) = (value.clone(), value.clone());
                    let (selected_key, pick_key) = (key.clone(), key.clone());
                    let selected = memo(move || read.get().as_ref() == Some(&selected_key));
                    text(move || item.with(|row| row.label.clone()))
                        .id(format!("{id}-tab-{key}"))
                        .role(Role::Tab)
                        .controls(format!("{id}-panel-{key}"))
                        .selected(selected)
                        .focus_preferred(selected)
                        .disabled(move || item.with(|row| row.disabled))
                        .focusable()
                        .min_w(0.0)
                        .min_h(32.0)
                        .when_touch(|p| p.min_h(crate::TOUCH_TARGET).min_w(crate::TOUCH_TARGET))
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .border()
                        .border_color(Paint::Color(Color::TRANSPARENT))
                        .text_sm()
                        .font_medium()
                        .text_color(Token::TextMuted)
                        .select_none()
                        .color_motion(Spring::SNAP)
                        .hover(|p| p.bg(Token::SurfaceHover).text_color(Token::Text))
                        .active(|p| p.bg(Token::SurfacePressed))
                        .style_when(State::SELECTED, |p| p.bg(Token::Surface).border_color(Token::Accent).text_color(Token::Text))
                        .focus_visible(|p| p.border_color(Token::Focus))
                        .style_when(State::DISABLED, |p| p.opacity(0.5))
                        .on_press(move || pick.set(Some(pick_key.clone())))
                },
            ))
            .into()
    })
}

/// A compact, equal-width choice of one stable option ID, with radio semantics.
///
/// `id` names the `RadioGroup`, its visible label is `{id}-label`, and choices are
/// `{id}-option-{option.id}`. Options use the same [`RadioOption`] data as [`crate::radio_group`];
/// descriptions are exposed to assistive technology without adding a second visible line.
/// Reordering and renaming preserve each choice's target. Missing selected IDs leave the
/// model untouched. Disabled choices stay visible and refuse activation.
/// `id` also keys the owner namespace for row work registered by model setters, as in [`tabs`].
///
/// Tab enters at the checked enabled choice, otherwise the remembered enabled choice,
/// otherwise the first enabled choice. Focus alone does not choose. Arrows wrap and choose;
/// Home/End choose the first/last enabled option. Press, Space and Enter choose too. This is a
/// radio choice, so it uses `checked`, while [`tabs`] uses `selected` and manual activation.
#[ui]
#[track_caller]
pub fn segmented(
    id: &str,
    label: impl Into<String>,
    value: impl Model<Option<String>>,
    options: impl Fn() -> Vec<RadioOption> + 'static,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    let value: Rc<dyn Model<Option<String>>> = Rc::new(value);
    scope(format!("Segmented[{id}]"), move || -> View {
        col()
            .id(id.clone())
            .role(Role::RadioGroup)
            .label(label.clone())
            .focus_group(FocusGroup::both().wrap().activate_on_navigation())
            .w_full()
            .min_w(0.0)
            .gap_1p5()
            .child(text(label).id(format!("{id}-label")).text_sm().font_medium().text_color(Token::Text))
            .child(row().w_full().min_w(0.0).gap_1().p_1().rounded_md().bg(Token::Surface).border().border_color(Token::Border).child(
                each(
                    move || options().into_iter().map(|option| (option.id.clone(), option)).collect::<Vec<_>>(),
                    move |key, item| {
                        let (read, pick) = (value.clone(), value.clone());
                        let (checked_key, pick_key) = (key.clone(), key.clone());
                        let checked = memo(move || read.get().as_ref() == Some(&checked_key));
                        row()
                            .id(format!("{id}-option-{key}"))
                            .role(Role::RadioButton)
                            .label(move || item.with(|row| row.label.clone()))
                            .description(move || item.with(|row| row.description.clone()))
                            .checked(checked)
                            .focus_preferred(checked)
                            .disabled(move || item.with(|row| row.disabled))
                            .focusable()
                            .flex_1()
                            .min_w(0.0)
                            .min_h(32.0)
                            .when_touch(|p| p.min_h(crate::TOUCH_TARGET))
                            .px_2()
                            .py_1p5()
                            .rounded_sm()
                            .border()
                            .border_color(Paint::Color(Color::TRANSPARENT))
                            .text_sm()
                            .font_medium()
                            .items_center()
                            .justify_center()
                            .text_color(Token::TextMuted)
                            .select_none()
                            .color_motion(Spring::SNAP)
                            .hover(|p| p.bg(Token::SurfaceHover).text_color(Token::Text))
                            .active(|p| p.bg(Token::SurfacePressed))
                            .style_when(State::CHECKED, |p| p.bg(Token::Background).text_color(Token::Text).shadow_sm())
                            .focus_visible(|p| p.border_color(Token::Focus))
                            .style_when(State::DISABLED, |p| p.opacity(0.5))
                            .child(text(move || item.with(|row| row.label.clone())).min_w(0.0).select_none().a11y_hidden())
                            .on_press(move || pick.set(Some(pick_key.clone())))
                    },
                ),
            ))
            .into()
    })
}
