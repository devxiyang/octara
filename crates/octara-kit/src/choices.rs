//! Controlled choices with shared identity and submission.

use crate::{Model, TOUCH_TARGET, popover, text_field};
use octara::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

/// An option for [`select`], [`combobox`], or [`tabs`](crate::tabs).
/// IDs are unique within the control and stay stable across label, order and
/// availability changes. The model stores the ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub disabled: bool,
}

impl Choice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), disabled: false }
    }

    /// Whether this choice is visible but unavailable for selection.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A choice from a tracked list of options. `value` holds the accepted option ID.
///
/// The trigger uses `id`, the popup is `{id}-popover`, the list is `{id}-list`, and rows use
/// `{id}-option-{choice.id}`. `label` names the control and supplies the fallback caption.
/// `trigger` builds the actual control once per mount from a tracked caption. Pass [`crate::button`]
/// for a standard trigger or return an element configured with styles, a tooltip, or a disabled state.
/// The list keeps focus as choices reorder or disappear. Arrow keys, Home and End move the
/// active option. Enter, Space or pressing an enabled row commits the choice.
#[ui]
#[track_caller]
pub fn select(
    id: &str,
    label: impl Into<String>,
    value: impl Model<Option<String>>,
    options: impl Fn() -> Vec<Choice> + 'static,
    trigger: impl FnOnce(ReadSignal<String>) -> El + 'static,
) -> View {
    picker(id.to_owned(), label.into(), value, options, trigger, false)
}

/// A searchable choice, with its selected option ID in `value`.
///
/// `label` names the control and supplies the fallback caption. The tracked option
/// source may change while open. Removing a selected option leaves the caller's model intact.
///
/// The trigger uses `id`, the popup is `{id}-popover`, the search field is `{id}-search`,
/// the list is `{id}-list`, and rows use `{id}-option-{choice.id}`.
/// `trigger` builds the actual control once per mount from a tracked caption. The picker supplies
/// its name, role, value, expanded state and open behavior on the returned element.
/// Search uses case-insensitive substring matching. Up and Down skip disabled choices.
/// The search field keeps focus and reports the active option separately from the selection.
/// Enter or pressing an enabled row commits a choice. Reopening clears the search and starts
/// at the selected enabled option or the first enabled option.
#[ui]
#[track_caller]
pub fn combobox(
    id: &str,
    label: impl Into<String>,
    value: impl Model<Option<String>>,
    options: impl Fn() -> Vec<Choice> + 'static,
    trigger: impl FnOnce(ReadSignal<String>) -> El + 'static,
) -> View {
    picker(id.to_owned(), label.into(), value, options, trigger, true)
}

#[track_caller]
fn picker(
    id: String,
    label: String,
    value: impl Model<Option<String>>,
    options: impl Fn() -> Vec<Choice> + 'static,
    trigger: impl FnOnce(ReadSignal<String>) -> El + 'static,
    searchable: bool,
) -> View {
    let value = Rc::new(value);
    let kind = if searchable { "Combobox" } else { "Select" };
    scope(format!("{kind}[{id}]"), move || {
        let open = state("open", || false);
        let active = state("active", || None::<String>);
        let query = searchable.then(|| state("query", String::new));
        let options = memo(options);
        let valid = memo(move || options.with(|options| unique_ids(options.iter().map(|option| option.id.as_str()))));
        let visible = memo(move || {
            let query = query.map_or_else(String::new, |query| query.with(|text| text.to_lowercase()));
            let valid = valid.get();
            options.with(|options| {
                Rc::new(
                    options
                        .iter()
                        // Give the keyed region the complete invalid snapshot so it reports duplicates.
                        .filter(|option| !valid || query.is_empty() || option.label.to_lowercase().contains(&query))
                        .map(|option| (option.id.clone(), option.clone()))
                        .collect::<Vec<_>>(),
                )
            })
        });
        let enabled = memo(move || {
            if !valid.get() {
                return Vec::new();
            }
            visible.with(|rows| rows.iter().filter(|(_, option)| !option.disabled).map(|(id, _)| id.clone()).collect::<Vec<_>>())
        });

        let selected = value.clone();
        watch(
            move || (enabled.get(), query.map(|query| query.get()), open.get(), selected.get()),
            move |(choices, query, shown, selected), previous| {
                if !shown {
                    return;
                }
                let opening = previous.is_none_or(|old| !old.2);
                if !opening && previous.is_some_and(|old| old.1 != *query) {
                    active.set(choices.first().cloned());
                } else if opening || !active.get().is_some_and(|id| choices.contains(&id)) {
                    let selected = selected.as_ref().filter(|id| choices.contains(id)).cloned();
                    active.set(selected.or_else(|| choices.first().cloned()));
                }
            },
        );
        let list_id = format!("{id}-list");
        let reveal_list = list_id.clone();
        let list_size = size_of(&list_id);
        watch(
            move || (open.get(), active.get(), visible.get(), list_size.get().1),
            move |(shown, active, _, height), _| {
                // Opening can assign the active key before the viewport's first layout.
                if *shown
                    && *height > 0.0
                    && let Some(active) = active
                {
                    reveal_row(&reveal_list, active.clone());
                }
            },
        );

        let submit = {
            let value = value.clone();
            move |id: String| {
                if open.get() && enabled.with(|ids| ids.contains(&id)) {
                    // The model's setter may close the window or otherwise retire this scope.
                    open.set(false);
                    value.set(Some(id));
                }
            }
        };
        let key_submit = submit.clone();
        let key = move |event: &KeyEvent| {
            if !event.modifiers.is_empty() {
                return false;
            }
            match event.key {
                Key::Up | Key::Down => {
                    let current = active.get();
                    let next = enabled.with(|ids| adjacent_id(ids, current.as_deref(), event.key == Key::Up).map(str::to_owned));
                    active.set(next);
                }
                Key::Home | Key::End if !searchable => {
                    active.set(enabled.with(|ids| if event.key == Key::Home { ids.first() } else { ids.last() }.cloned()));
                }
                Key::Enter | Key::Space if event.key == Key::Enter || !searchable => {
                    if let Some(id) = active.get() {
                        key_submit(id);
                    }
                }
                _ => return false,
            }
            true
        };

        let caption = {
            let (value, label) = (value.clone(), label.clone());
            memo(move || {
                if !valid.get() {
                    return label.clone();
                }
                let selected = value.get();
                options.with(|options| {
                    options
                        .iter()
                        .find(|option| Some(&option.id) == selected.as_ref())
                        .map_or_else(|| label.clone(), |option| option.label.clone())
                })
            })
        };
        let trigger = trigger(caption).role(Role::ComboBox).label(label.clone()).a11y_value(caption).controls(list_id.clone());
        let popup_id = id.clone();
        popover(
            &id,
            (
                move || open.get(),
                move |shown| {
                    if shown && !open.get() {
                        if let Some(query) = query {
                            query.set(String::new());
                        }
                        active.set(None);
                    }
                    open.set(shown);
                },
            ),
            Placement::BELOW.fit(true),
            trigger,
            move || {
                let (row_id, value, submit) = (popup_id.clone(), value.clone(), submit.clone());
                let active_id = popup_id.clone();
                let active_descendant = move || active.get().map(|id| format!("{active_id}-option-{id}"));
                let app = App::current();
                let theme = app.theme();
                let estimated_height = (theme.text_of(TextStep::Sm) * theme.line_height).ceil() + 3.0 * theme.spacing;
                let estimated_height = if app.target_platform().is_touch() { estimated_height.max(TOUCH_TARGET) } else { estimated_height };
                let list =
                    col().id(list_id.clone()).role(Role::ListBox).label(label.clone()).gap_0p5().max_h(240.0).overflow_y_scroll().child(
                        virtual_each(
                            move || visible.with(|rows| rows.as_ref().clone()),
                            move |_| estimated_height,
                            move |id, row| {
                                let (selected, submit) = (value.clone(), submit.clone());
                                let (selected_id, active_id, submitted_id) = (id.clone(), id.clone(), id.clone());
                                text(move || row.with(|option| option.label.clone()))
                                    .id(format!("{row_id}-option-{id}"))
                                    .role(Role::ListBoxOption)
                                    .preserve_focus_on_press(true)
                                    .w_full()
                                    .min_w(0.0)
                                    .text_ellipsis()
                                    .flex_shrink_0()
                                    .px_3()
                                    .py_1p5()
                                    .when_touch(|p| p.min_h(TOUCH_TARGET))
                                    .rounded_sm()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(Token::Text)
                                    .disabled(move || row.with(|option| option.disabled))
                                    .selected(move || selected.get().as_deref() == Some(selected_id.as_str()))
                                    .style_when(State::SELECTED, |p| p.text_color(Token::Accent))
                                    .style_when(State::DISABLED, |p| p.opacity(0.5))
                                    .hover(|p| p.bg(Token::SurfaceHover))
                                    .style_with(move |p| {
                                        p.bg(if active.get().as_ref() == Some(&active_id) { Token::SurfacePressed } else { Token::Surface })
                                    })
                                    .on_press(move || submit(submitted_id.clone()))
                            },
                        ),
                    );
                let panel = if let Some(query) = query {
                    let search = text_field(query)
                        .id(format!("{popup_id}-search"))
                        .label(format!("Search {label}"))
                        .w_full()
                        .autofocus()
                        .active_descendant(active_descendant)
                        .on_key(key.clone());
                    col().w(260.0).gap_2().child(search).child(list)
                } else {
                    col().min_w(160.0).child(list.focusable().autofocus().active_descendant(active_descendant).on_key(key.clone()))
                };
                let empty_id = format!("{popup_id}-empty");
                panel.child(show(
                    move || visible.with(|rows| rows.is_empty()),
                    move || {
                        text(if searchable { "No results" } else { "No options" })
                            .id(empty_id.clone())
                            .px_3()
                            .py_2()
                            .text_sm()
                            .text_color(Token::TextMuted)
                    },
                ))
            },
        )
    })
}

pub(super) fn unique_ids<'a>(mut ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = HashSet::new();
    ids.all(|id| seen.insert(id))
}

/// Wrap within already enabled IDs. An absent/stale active ID starts at the corresponding
/// edge; an empty list has no destination. This never changes selection or component state.
pub(super) fn adjacent_id<'a>(ids: &'a [String], active: Option<&str>, backward: bool) -> Option<&'a str> {
    if ids.is_empty() {
        return None;
    }
    let at = active.and_then(|id| ids.iter().position(|item| item == id));
    let next = match (backward, at) {
        (true, Some(0) | None) => ids.len() - 1,
        (true, Some(at)) => at - 1,
        (false, Some(at)) => (at + 1) % ids.len(),
        (false, None) => 0,
    };
    Some(ids[next].as_str())
}
