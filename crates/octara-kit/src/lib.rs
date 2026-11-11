//! Themable components for Octara, built only on octara's public primitives.
//!
//! Every component returns an [`El`] (or a [`View`]), so callers keep chaining `id`, styles,
//! `disabled` and handlers. Controlled components take a [`Model`]: a [`Signal`], or a
//! `(getter, setter)` pair when the value lives elsewhere (for example in a list entry).
//! Colors come from theme [`Token`]s; semantics (role, checked, expanded, selected) are set for
//! assistive technology and for tests.

use octara::prelude::*;
use std::rc::Rc;

// ---- models ----

/// A value a component reads (tracked) and writes.
pub trait Model<T>: 'static {
    fn get(&self) -> T;
    fn set(&self, value: T);
}

impl<T: Clone + 'static> Model<T> for Signal<T> {
    fn get(&self) -> T {
        Signal::get(self)
    }
    fn set(&self, value: T) {
        Signal::set(self, value)
    }
}

impl<T, G, S> Model<T> for (G, S)
where
    G: Fn() -> T + 'static,
    S: Fn(T) + 'static,
{
    fn get(&self) -> T {
        (self.0)()
    }
    fn set(&self, value: T) {
        (self.1)(value)
    }
}

// ---- shared looks ----

/// Focus ring and disabled look shared by every interactive component.
fn interactive(e: El) -> El {
    // Hover, press and focus shades ease in (Rune's kit: colors on a quick spring).
    e.color_motion(Spring::SNAP)
        .focusable()
        .border()
        .border_color(Token::Border)
        .focus_visible(|p| p.border_color(Token::Focus))
        .style_when(State::DISABLED, |p| p.opacity(0.5))
}

// ---- button ----

/// A secondary button: surface background, border, hover and pressed shades.
#[track_caller]
pub fn button(label: impl IntoProp<String>) -> El {
    interactive(text(label).role(Role::Button))
        .px_3()
        .py_1p5()
        .rounded_md()
        .text_sm()
        .font_medium()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .hover(|p| p.bg(Token::SurfaceHover))
        .active(|p| p.bg(Token::SurfacePressed))
}

/// The main action of a view: accent background.
#[track_caller]
pub fn primary_button(label: impl IntoProp<String>) -> El {
    button(label)
        .bg(Token::Accent)
        .text_color(Token::AccentText)
        .border_color(Token::Accent)
        .hover(|p| p.bg(Token::AccentHover))
        .active(|p| p.bg(Token::AccentHover))
}

/// A low-emphasis button: no background until hovered.
#[track_caller]
pub fn ghost_button(label: impl IntoProp<String>) -> El {
    button(label).bg(Paint::Color(Color::TRANSPARENT)).border_color(Paint::Color(Color::TRANSPARENT))
}

/// A button that stays selected while `selected` holds (tabs, segmented controls, filters).
#[track_caller]
pub fn toggle_button(label: impl IntoProp<String>, selected: impl Fn() -> bool + 'static) -> El {
    ghost_button(label)
        .selected(selected)
        .style_when(State::SELECTED, |p| p.bg(Token::Surface).border_color(Token::Border).text_color(Token::Accent))
}

// ---- checkbox and switch ----

fn toggle_model(model: impl Model<bool>) -> (impl Fn() -> bool + 'static, impl Fn() + 'static) {
    let m = Rc::new(model);
    let r = m.clone();
    (move || r.get(), move || m.set(!m.get()))
}

/// A checkbox bound to `checked`; `Event::Toggle` flips it.
#[track_caller]
pub fn checkbox(checked: impl Model<bool>) -> El {
    let (get, flip) = toggle_model(checked);
    interactive(el().role(Role::CheckBox))
        .size_4()
        .rounded_sm()
        .bg(Token::Background)
        .style_when(State::CHECKED, |p| p.bg(Token::Accent).border_color(Token::Accent))
        .checked(get)
        .on_toggle(flip)
}

/// An on/off switch bound to `on`; `Event::Toggle` flips it.
#[track_caller]
pub fn switch(on: impl Model<bool>) -> El {
    let (get, flip) = toggle_model(on);
    let thumb = el().size_4().rounded_full().bg(Token::Background).a11y_hidden();
    interactive(el().child(thumb).role(Role::Switch))
        .w_8()
        .h_5()
        .p(1.0)
        .items_center()
        .rounded_full()
        .bg(Token::Border)
        .style_when(State::CHECKED, |p| p.bg(Token::Accent).justify_end())
        .checked(get)
        .on_toggle(flip)
}

// ---- text field ----

/// A single-line text field bound to `value`.
#[track_caller]
pub fn text_field(value: impl Model<String>) -> El {
    let m = Rc::new(value);
    let r = m.clone();
    interactive(text_input())
        .px_3()
        .py_1p5()
        .rounded_md()
        .text_sm()
        .bg(Token::Background)
        .text_color(Token::Text)
        .value(move || r.get())
        .on_input(move |v| m.set(v))
}

// ---- select ----

/// A dropdown choosing one of `options` (value, label), bound to `value`.
///
/// `id` names the trigger and keys the component, so its open state survives hot reload. The
/// trigger has role `ComboBox` and `expanded`; pressing it opens a `ListBox` below it (flipped
/// above when there is no room) with options `{id}-option-{index}`, the current one `selected`.
#[track_caller]
pub fn select<T: Clone + PartialEq + 'static>(
    id: &str,
    value: impl Model<T>,
    options: impl IntoIterator<Item = (T, impl Into<String>)>,
) -> View {
    let id = id.to_owned();
    let options: Rc<Vec<(T, String)>> = Rc::new(options.into_iter().map(|(v, l)| (v, l.into())).collect());
    let value: Rc<dyn Model<T>> = Rc::new(value);
    component_keyed("Select", id.clone(), move || {
        let open = state("open", || false);
        let label = {
            let (value, options) = (value.clone(), options.clone());
            move || {
                let v = value.get();
                options.iter().find(|(o, _)| *o == v).map(|(_, l)| l.clone()).unwrap_or_default()
            }
        };
        let list = {
            let id = id.clone();
            move || {
                let items: Vec<View> = options
                    .iter()
                    .enumerate()
                    .map(|(i, (v, l))| {
                        let (value, pick, current) = (value.clone(), v.clone(), v.clone());
                        let probe = value.clone();
                        ghost_button(l.as_str())
                            .id(format!("{id}-option-{i}"))
                            .role(Role::ListBoxOption)
                            .w_full()
                            .rounded_sm()
                            .selected(move || probe.get() == current)
                            .style_when(State::SELECTED, |p| p.text_color(Token::Accent))
                            .on_press(move || {
                                value.set(pick.clone());
                                open.set(false);
                            })
                            .into()
                    })
                    .collect();
                col()
                    .children(items)
                    .role(Role::ListBox)
                    .id(format!("{id}-list"))
                    .min_w(160.0)
                    .p_1()
                    .gap_0p5()
                    .rounded_md()
                    .border()
                    .border_color(Token::Border)
                    .bg(Token::Surface)
                    .shadow_lg()
                    // Arrow keys move between options; Escape or a press outside closes the list
                    // and focus goes back to the select.
                    .focus_group()
                    .modal(true, move || open.set(false))
                    .into()
            }
        };
        button(label.clone())
            .id(id.clone())
            .role(Role::ComboBox)
            .value(label)
            .expanded(move || open.get())
            .on_press(move || open.update(|o| *o = !*o))
            .overlay(move || open.get(), Placement::BELOW, list)
            .into()
    })
}
