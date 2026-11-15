//! A searchable, externally controlled choice with stable option identities.

use crate::{Model, button, popover, text_field};
use octara::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

/// One choice in a [`combobox`]. IDs must be unique and remain stable when options move or
/// their labels change. The selected model stores this ID, rather than the display label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComboboxOption {
    pub id: String,
    pub label: String,
    pub disabled: bool,
}

impl ComboboxOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), disabled: false }
    }

    /// Whether this choice is unavailable. It remains visible in matching search results.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Clone, PartialEq)]
struct OptionRow {
    option: ComboboxOption,
    order: usize,
    matches: bool,
}

/// A searchable choice, with its selected option ID in `value`.
///
/// `label` names the control and is shown when the selected ID is absent from `options`.
/// `options` is tracked, so labels, availability, insertion, removal and ordering can change
/// while the popup is open. Removing a selected option leaves the caller's value intact.
///
/// `id` names the trigger and keys the component's named state. Its modal popup is
/// `{id}-popover`, search field `{id}-search`, list `{id}-list`, and each option
/// `{id}-option-{option.id}`. `{id}-empty` is shown when no options match.
///
/// Search is case-insensitive substring matching. Up/Down move the active option, skipping
/// disabled choices; Enter selects it. Search keeps keyboard focus, with its active descendant
/// identifying the highlighted option separately from the selected value. Escape and a press
/// outside dismiss without choosing. Reopening clears the query and starts at the selected
/// enabled option, or the first enabled option. Editing the query starts at the first enabled
/// match. This is an ordinary retained list for finite menus (including hundreds of choices),
/// rather than a virtual list or an asynchronous search source.
/// The trigger fills its containing box and truncates long labels. Use an ordinary wrapper
/// element to constrain its width or attach a tooltip; the full selected label remains its value.
#[component]
#[track_caller]
pub fn combobox(
    id: &str,
    label: impl Into<String>,
    value: impl Model<Option<String>>,
    options: impl Fn() -> Vec<ComboboxOption> + 'static,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    let value: Rc<dyn Model<Option<String>>> = Rc::new(value);
    component_keyed("Combobox", id.clone(), move || {
        let open = state("open", || false);
        let query = state("query", String::new);
        let highlight = state("highlight", || None::<String>);
        let options = memo(options);
        let rows = state_list("options", Vec::<OptionRow>::new, |row: &OptionRow| row.option.id.clone());
        let matches = rows.filter(|row| row.matches).sort_by(|row| row.order);
        let enabled = memo(move || {
            let query = query.get().to_lowercase();
            options
                .get()
                .into_iter()
                .filter(|option| !option.disabled && option.label.to_lowercase().contains(&query))
                .map(|option| option.id)
                .collect::<Vec<_>>()
        });

        // Keep keyed rows rather than rebuilding the popup when its query or source changes.
        // Only the list observes these patches; the retained search field keeps its editor/IME.
        let current = value.clone();
        watch(
            move || (options.get(), query.get(), open.get(), current.get()),
            move |(options, query, is_open, selected), previous| {
                let query_changed = previous.is_some_and(|old| old.1 != *query);
                let keys: HashSet<_> = options.iter().map(|option| option.id.as_str()).collect();
                rows.retain(|row| keys.contains(row.option.id.as_str()));
                let query = query.to_lowercase();
                let mut seen = HashSet::new();
                for (order, option) in options.iter().enumerate() {
                    let row = OptionRow { option: option.clone(), order, matches: option.label.to_lowercase().contains(&query) };
                    if !seen.insert(option.id.clone()) {
                        // Let the list report its ordinary DUPLICATE_KEY diagnostic.
                        rows.push(row);
                    } else if let Some(old) = rows.item(&option.id) {
                        if old.get_untracked() != row {
                            rows.update(&option.id, |old| *old = row);
                        }
                    } else {
                        rows.push(row);
                    }
                }
                if !is_open {
                    return;
                }
                let choices = enabled.get();
                let opening = previous.is_some_and(|old| !old.2);
                let active = highlight.get();
                if query_changed && !opening {
                    highlight.set(choices.first().cloned());
                } else if opening || !active.as_ref().is_some_and(|id| choices.contains(id)) {
                    let selected = selected.as_ref().filter(|id| choices.contains(id)).cloned();
                    highlight.set(selected.or_else(|| choices.first().cloned()));
                }
            },
        );

        let reveal_id = id.clone();
        watch(
            move || (open.get(), highlight.get(), options.get(), query.get()),
            move |(is_open, active, _, _), _| {
                if *is_open && let Some(active) = active {
                    scroll_into_view(&format!("{reveal_id}-option-{active}"));
                }
            },
        );

        let caption = {
            let (value, label) = (value.clone(), label.clone());
            move || {
                let selected = value.get();
                options
                    .get()
                    .into_iter()
                    .find(|option| Some(&option.id) == selected.as_ref())
                    .map(|option| option.label)
                    .unwrap_or_else(|| label.clone())
            }
        };
        let trigger = button(caption.clone()).label(label.clone()).value(caption).w_full().min_w(0.0).truncate();
        let popup_id = id.clone();
        let popup_label = label.clone();
        let popup_value = value.clone();
        popover(
            &id,
            (
                move || open.get(),
                move |shown| {
                    if shown && !open.get() {
                        query.set(String::new());
                        highlight.set(None);
                    }
                    open.set(shown);
                },
            ),
            Placement::BELOW.fit(true),
            trigger,
            move || {
                let active_id = popup_id.clone();
                let pick = popup_value.clone();
                let search = text_field(query)
                    .id(format!("{popup_id}-search"))
                    .label(format!("Search {popup_label}"))
                    .w_full()
                    .autofocus()
                    .active_descendant(move || highlight.get().map(|key| format!("{active_id}-option-{key}")))
                    .on_key(move |key| {
                        // Modified arrows, Home/End and other text-editing keys belong to the
                        // editor. The runtime gives composition Enter/Escape precedence here.
                        if !key.modifiers.is_empty() {
                            return false;
                        }
                        match key.key {
                            Key::Up | Key::Down => {
                                let choices = enabled.get();
                                let current = highlight.get();
                                let index = current.as_ref().and_then(|id| choices.iter().position(|choice| choice == id));
                                let next = match (key.key, index) {
                                    (_, _) if choices.is_empty() => None,
                                    (Key::Down, Some(index)) => Some((index + 1) % choices.len()),
                                    (Key::Up, Some(index)) => Some((index + choices.len() - 1) % choices.len()),
                                    (Key::Up, None) => Some(choices.len() - 1),
                                    _ => Some(0),
                                };
                                highlight.set(next.map(|index| choices[index].clone()));
                                true
                            }
                            Key::Enter => {
                                if let Some(active) = highlight.get().filter(|id| enabled.get().contains(id)) {
                                    pick.set(Some(active));
                                    open.set(false);
                                }
                                true
                            }
                            _ => false,
                        }
                    });
                let list_id = popup_id.clone();
                let row_value = popup_value.clone();
                let list = col()
                    .id(format!("{popup_id}-list"))
                    .role(Role::ListBox)
                    .label(popup_label.clone())
                    .gap_0p5()
                    .max_h(240.0)
                    .overflow_y_scroll()
                    .child(each(
                        move || matches.source(),
                        move |key, row| {
                            let (selected, pick) = (row_value.clone(), row_value.clone());
                            let (selected_key, active_key, pick_key) = (key.clone(), key.clone(), key.clone());
                            button(move || row.with(|row| row.option.label.clone()))
                                .id(format!("{list_id}-option-{key}"))
                                .role(Role::ListBoxOption)
                                .tab_index(-1)
                                .w_full()
                                .min_w(0.0)
                                .truncate()
                                .rounded_sm()
                                .border_color(Paint::Color(Color::TRANSPARENT))
                                .disabled(move || row.with(|row| row.option.disabled))
                                .selected(move || selected.get().as_ref() == Some(&selected_key))
                                .style_when(State::SELECTED, |p| p.text_color(Token::Accent))
                                .style_with(move |p| {
                                    p.bg(if highlight.get().as_ref() == Some(&active_key) { Token::SurfacePressed } else { Token::Surface })
                                })
                                .on_press(move || {
                                    pick.set(Some(pick_key.clone()));
                                    open.set(false);
                                })
                                .into()
                        },
                    ));
                let empty_id = popup_id.clone();
                col()
                    .w(260.0)
                    .gap_2()
                    .child(search)
                    .child(list)
                    .child(show(
                        move || matches.is_empty(),
                        move || {
                            text("No results").id(format!("{empty_id}-empty")).px_3().py_2().text_sm().text_color(Token::TextMuted).into()
                        },
                    ))
                    .into()
            },
        )
        .role(Role::ComboBox)
        .into()
    })
}
