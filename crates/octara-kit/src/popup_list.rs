//! Suggestions owned by an existing editor, without moving keyboard focus into the popup.

use crate::Model;
use octara::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

/// A suggestion with a stable identity. Filtering belongs to the application; labels may repeat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupItem {
    pub id: String,
    pub label: String,
    pub description: String,
    pub disabled: bool,
}

impl PopupItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), description: String::new(), disabled: false }
    }
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Clone, PartialEq)]
struct Row {
    item: PopupItem,
    order: usize,
}

/// Attach suggestions to `editor`, whose text model and input handler remain the caller's.
///
/// `id` names the editor, `{id}-list` the listbox and `{id}-option-{item.id}` each row.
/// The caller controls `open` and supplies already filtered items. Up/Down wrap among enabled
/// suggestions; Down also opens a closed list. Enter chooses, Escape closes, and Tab closes
/// before continuing normal focus navigation. Modified keys and Home/End belong to the editor.
/// The component owns the editor's key and blur handlers, expanded and active-descendant props.
///
/// The list is nonmodal. A press on a suggestion preserves the editor's focus and selection;
/// a press elsewhere can focus that control and closes the list on blur. Choosing calls
/// `on_choose` with the stable ID and closes; it never writes the editor's text itself.
/// During IME composition, keyboard navigation belongs to the IME and pointer choices do
/// nothing. Rows are retained through option updates; closing disposes only the popup.
/// `selected` denotes the pending suggestion, not a committed application value.
#[component]
#[track_caller]
pub fn popup_list(
    id: &str,
    label: impl Into<String>,
    open: impl Model<bool>,
    editor: El,
    items: impl Fn() -> Vec<PopupItem> + 'static,
    on_choose: impl Fn(String) + 'static,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    let open: Rc<dyn Model<bool>> = Rc::new(open);
    let choose: Rc<dyn Fn(String)> = Rc::new(on_choose);
    component_keyed("PopupList", id.clone(), move || {
        let active = state("active", || None::<String>);
        let items = memo(items);
        let rows = state_list("items", Vec::<Row>::new, |row: &Row| row.item.id.clone());
        let ordered = rows.sort_by(|row| row.order);
        let enabled = memo(move || items.get().into_iter().filter(|item| !item.disabled).map(|item| item.id).collect::<Vec<_>>());
        let shown = open.clone();
        watch(
            move || (items.get(), shown.get()),
            move |(items, shown), previous| {
                let keys: HashSet<_> = items.iter().map(|item| item.id.as_str()).collect();
                rows.retain(|row| keys.contains(row.item.id.as_str()));
                let mut seen = HashSet::new();
                for (order, item) in items.iter().enumerate() {
                    let row = Row { item: item.clone(), order };
                    if !seen.insert(item.id.clone()) {
                        rows.push(row); // The ordinary stable-list duplicate-key diagnostic.
                    } else if let Some(old) = rows.item(&item.id) {
                        if old.get_untracked() != row {
                            rows.update(&item.id, |old| *old = row);
                        }
                    } else {
                        rows.push(row);
                    }
                }
                if *shown {
                    let choices = enabled.get();
                    if previous.is_none_or(|old| !old.1) || !active.get().is_some_and(|id| choices.contains(&id)) {
                        active.set(choices.first().cloned());
                    }
                }
            },
        );
        let (revealed, reveal_id) = (open.clone(), id.clone());
        watch(
            move || (revealed.get(), active.get(), items.get()),
            move |(shown, active, _), _| {
                if *shown && let Some(active) = active {
                    scroll_into_view(&format!("{reveal_id}-option-{active}"));
                }
            },
        );
        let pick: Rc<dyn Fn(String)> = {
            let (open, id) = (open.clone(), id.clone());
            Rc::new(move |key| {
                if open.get() && !is_composing(&id) && enabled.get().contains(&key) {
                    // Retire the popup before application callbacks can open another one.
                    open.set(false);
                    choose(key);
                }
            })
        };
        let (expanded, related, blur, keys, shown) = (open.clone(), open.clone(), open.clone(), open.clone(), open);
        let (related_id, panel_id) = (id.clone(), id.clone());
        let key_pick = pick.clone();
        editor
            .id(id)
            .expanded(move || expanded.get())
            .active_descendant(move || related.get().then(|| active.get()).flatten().map(|key| format!("{related_id}-option-{key}")))
            .on_blur(move || blur.set(false))
            .on_key(move |event| {
                if event.key == Key::Tab && (event.modifiers.is_empty() || event.modifiers == Modifiers::SHIFT) {
                    keys.set(false);
                    return false;
                }
                if !event.modifiers.is_empty() {
                    return false;
                }
                if !keys.get() {
                    if event.key == Key::Down {
                        keys.set(true);
                        return true;
                    }
                    return false;
                }
                match event.key {
                    Key::Up | Key::Down => {
                        let choices = enabled.get();
                        if !choices.is_empty() {
                            let at = active.get().and_then(|id| choices.iter().position(|item| item == &id));
                            let at = match (event.key, at) {
                                (Key::Up, Some(at)) => (at + choices.len() - 1) % choices.len(),
                                (Key::Down, Some(at)) => (at + 1) % choices.len(),
                                (Key::Up, None) => choices.len() - 1,
                                _ => 0,
                            };
                            active.set(Some(choices[at].clone()));
                        }
                        true
                    }
                    Key::Enter => {
                        if let Some(id) = active.get() {
                            key_pick(id);
                        }
                        true
                    }
                    Key::Escape => {
                        keys.set(false);
                        true
                    }
                    Key::Tab => {
                        keys.set(false);
                        false
                    }
                    _ => false,
                }
            })
            .overlay(
                move || shown.get(),
                Placement::BELOW.fit(true),
                move || {
                    let (row_id, pick) = (panel_id.clone(), pick.clone());
                    col()
                        .id(format!("{panel_id}-list"))
                        .role(Role::ListBox)
                        .label(label.clone())
                        .preserve_focus_on_press(true)
                        .w(280.0)
                        .max_h(240.0)
                        .style_with(|p| p.max_w((viewport().get().0 - 16.0).max(0.0)))
                        .overflow_y_scroll()
                        .p_1()
                        .gap_0p5()
                        .rounded_md()
                        .border()
                        .border_color(Token::Border)
                        .bg(Token::Surface)
                        .shadow_lg()
                        .child(each(
                            move || ordered.source(),
                            move |key, item| {
                                let (selected, chosen, description_key, pick) = (key.clone(), key.clone(), key.clone(), pick.clone());
                                col()
                                    .id(format!("{row_id}-option-{key}"))
                                    .role(Role::ListBoxOption)
                                    .label(move || item.with(|row| row.item.label.clone()))
                                    .description(move || item.with(|row| row.item.description.clone()))
                                    .disabled(move || item.with(|row| row.item.disabled))
                                    .selected(move || active.get().as_ref() == Some(&selected))
                                    .w_full()
                                    .min_w(0.0)
                                    .min_h(32.0)
                                    .when_touch(|p| p.min_h(crate::TOUCH_TARGET))
                                    .px_3()
                                    .py_1p5()
                                    .gap_0p5()
                                    .rounded_sm()
                                    .text_sm()
                                    .text_color(Token::Text)
                                    .hover(|p| p.bg(Token::SurfaceHover))
                                    .style_when(State::SELECTED, |p| p.bg(Token::SurfacePressed))
                                    .style_when(State::DISABLED, |p| p.opacity(0.5))
                                    .child(text(move || item.with(|row| row.item.label.clone())).select_none())
                                    .child(show(
                                        move || item.with(|row| !row.item.description.trim().is_empty()),
                                        move || {
                                            let description_key = description_key.clone();
                                            text(move || item.with(|row| row.item.description.clone()))
                                                .text_xs()
                                                .style_with(move |p| {
                                                    p.text_color(if active.get().as_ref() == Some(&description_key) {
                                                        Token::Text
                                                    } else {
                                                        Token::TextMuted
                                                    })
                                                })
                                                .select_none()
                                                .into()
                                        },
                                    ))
                                    .on_press(move || pick(chosen.clone()))
                                    .into()
                            },
                        ))
                        .child(show(move || items.get().is_empty(), {
                            let id = panel_id.clone();
                            move || {
                                text("No suggestions").id(format!("{id}-empty")).px_3().py_2().text_sm().text_color(Token::TextMuted).into()
                            }
                        }))
                        .into()
                },
            )
            .into()
    })
}
