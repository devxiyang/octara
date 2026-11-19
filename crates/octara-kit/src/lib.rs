//! Themable components for Octara, built only on octara's public primitives.
//!
//! Every component returns an [`El`] (or a [`View`]), so callers keep chaining `id`, styles,
//! `disabled` and handlers. Controlled components take a [`Model`]: a [`Signal`], or a
//! `(getter, setter)` pair when the value lives elsewhere (for example in a list entry).
//! Structural wrappers such as [`field`] return their container; configure the supplied
//! control's handlers and disabled state before wrapping it.
//! Colors come from theme [`Token`]s; semantics (role, checked, expanded, selected) are set for
//! assistive technology and for tests.

use octara::prelude::*;
use std::rc::Rc;

mod choices;
mod dialog;
mod forms;
pub mod markdown;
mod menus;
mod navigation;
mod popover;
mod popup_list;
mod range;
mod status;
mod structure;

pub use choices::{Choice, combobox, select};
pub use dialog::{DialogOptions, dialog};
pub use forms::{FieldInfo, RadioOption, field, radio_group};
pub use menus::menu_button;
pub use navigation::{segmented, tabs};
pub use popover::popover;
pub use popup_list::{PopupItem, popup_list};
pub use range::{normalize_number, number_field, parse_number, progress, slider};
pub use status::{callout, chip, removable_chip, spinner};
pub use structure::{collapsible, list, list_header, list_item, split, splitter};

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

/// Controls are sized for the platform, as native controls are (Exact's rule): pointer sizes on
/// desktop, at least 44 × 44 on touch.
const TOUCH_TARGET: f32 = 44.0;

/// Focus ring and disabled look shared by every interactive component.
fn interactive<C>(e: El<C>) -> El<C> {
    // Hover, press and focus shades ease in (Rune's kit: colors on a quick spring).
    e.color_motion(Spring::SNAP)
        .focusable()
        .border()
        .border_color(Token::Border)
        .focus_visible(|p| p.border_color(Token::Focus))
        .style_when(State::DISABLED, |p| p.opacity(0.5))
}

// ---- button ----

/// Applies the Kit button role, keyboard focus ring and disabled styling to an element.
/// Chain the button's ID, accessible label, visuals and press handler on the returned element.
pub fn button_root(element: El) -> El {
    interactive(element.role(Role::Button))
}

/// A secondary button: surface background, border, hover and pressed shades.
#[ui]
#[track_caller]
pub fn button(label: impl IntoText) -> El {
    button_root(text(label))
        .px_3()
        .py_1p5()
        // 20 px of text and 12 px above and below: 44 on touch.
        .when_touch(|p| p.px_4().py_3().min_w(TOUCH_TARGET))
        .rounded_md()
        .text_sm()
        .font_medium()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .hover(|p| p.bg(Token::SurfaceHover))
        .active(|p| p.bg(Token::SurfacePressed))
}

/// The main action of a view: accent background.
#[ui]
#[track_caller]
pub fn primary_button(label: impl IntoText) -> El {
    button(label)
        .bg(Token::Accent)
        .text_color(Token::AccentText)
        .border_color(Token::Accent)
        .hover(|p| p.bg(Token::AccentHover))
        .active(|p| p.bg(Token::AccentHover))
}

/// A low-emphasis button: no background until hovered.
#[ui]
#[track_caller]
pub fn ghost_button(label: impl IntoText) -> El {
    button(label).bg(Paint::Color(Color::TRANSPARENT)).border_color(Paint::Color(Color::TRANSPARENT))
}

/// A button that stays selected while `selected` holds (tabs, segmented controls, filters).
#[ui]
#[track_caller]
pub fn toggle_button(label: impl IntoText, selected: impl Fn() -> bool + 'static) -> El {
    ghost_button(label)
        .selected(selected)
        .style_when(State::SELECTED, |p| p.bg(Token::Surface).border_color(Token::Border).text_color(Token::Accent))
}

/// A button showing only `icon`, named `label`: its name for assistive technology and its
/// tooltip. Quiet until hovered (a toolbar's, a message's actions).
#[ui]
#[track_caller]
pub fn icon_button(icon: octara_primitives::icon::Icon, label: impl Into<String>) -> El {
    let label = label.into();
    button_root(el())
        .child(octara::view::icon(icon).a11y_hidden())
        .label(label.clone())
        .tooltip(label)
        .items_center()
        .justify_center()
        .size(28.0)
        .flex_shrink_0()
        .when_touch(|p| p.size(TOUCH_TARGET))
        .rounded_md()
        .bg(Paint::Color(Color::TRANSPARENT))
        .border_color(Paint::Color(Color::TRANSPARENT))
        .text_color(Token::TextMuted)
        .hover(|p| p.bg(Token::SurfaceHover).text_color(Token::Text))
        .active(|p| p.bg(Token::SurfacePressed))
}

// ---- checkbox and switch ----

fn toggle_model(model: impl Model<bool>) -> (impl Fn() -> bool + 'static, impl Fn() + 'static) {
    let m = Rc::new(model);
    let r = m.clone();
    (move || r.get(), move || m.set(!m.get()))
}

/// The press area of a small control: the element that is focused, pressed and named, around
/// the control's visual. It hugs the visual with a pointer and grows to 44 × 44 on touch (the
/// visual stays its size, centered). Its border shows only as the focus ring.
fn target(role: Role, visual: El) -> El {
    el().role(role)
        .child(visual)
        .items_center()
        .justify_center()
        .color_motion(Spring::SNAP)
        .focusable()
        .border()
        .border_color(Paint::Color(Color::TRANSPARENT))
        .rounded_md()
        .focus_visible(|p| p.border_color(Token::Focus))
        .style_when(State::DISABLED, |p| p.opacity(0.5))
        .when_touch(|p| p.min_w(TOUCH_TARGET).min_h(TOUCH_TARGET))
}

/// A checkbox bound to `checked`; `Event::Toggle` flips it. An application `on_toggle`
/// runs after the model setter and preserves the binding.
#[ui]
#[track_caller]
pub fn checkbox(checked: impl Model<bool>) -> El {
    let (get, flip) = toggle_model(checked);
    let get = Rc::new(get);
    let shown = get.clone();
    // The box itself carries `checked` too, for its look; it is hidden from assistive technology
    // (the press area is the checkbox there).
    let square = el()
        .size_4()
        .rounded_sm()
        .border()
        .border_color(Token::Border)
        .bg(Token::Background)
        .color_motion(Spring::SNAP)
        .checked(move || shown())
        .style_when(State::CHECKED, |p| p.bg(Token::Accent).border_color(Token::Accent))
        .a11y_hidden();
    target(Role::CheckBox, square).checked(move || get()).behavior(|b| b.on_toggle(flip))
}

/// An on/off switch bound to `on`; `Event::Toggle` flips it. An application `on_toggle`
/// runs after the model setter and preserves the binding.
#[ui]
#[track_caller]
pub fn switch(on: impl Model<bool>) -> El {
    let (get, flip) = toggle_model(on);
    let get = Rc::new(get);
    let shown = get.clone();
    let thumb = el().size_4().rounded_full().bg(Token::Background);
    let track = el()
        .child(thumb)
        .w_8()
        .h_5()
        .p(2.0)
        .items_center()
        .rounded_full()
        .bg(Token::Border)
        .color_motion(Spring::SNAP)
        .checked(move || shown())
        .style_when(State::CHECKED, |p| p.bg(Token::Accent).justify_end())
        .a11y_hidden();
    target(Role::Switch, track).checked(move || get()).behavior(|b| b.on_toggle(flip))
}

// ---- text field ----

/// A single-line text field bound to `value`. The model is updated before the replaceable
/// `on_input` application callback; adding that callback preserves the binding.
#[ui]
#[track_caller]
pub fn text_field(value: impl Model<String>) -> TextInput {
    let m = Rc::new(value);
    let r = m.clone();
    interactive(text_input())
        .px_3()
        .py_1p5()
        .when_touch(|p| p.py_3().min_h(TOUCH_TARGET))
        .rounded_md()
        .text_sm()
        .bg(Token::Background)
        .text_color(Token::Text)
        .value(move || r.get())
        .behavior(|b| b.on_input(move |v| m.set(v)))
}
