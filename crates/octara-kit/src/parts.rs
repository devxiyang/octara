//! Kit styling for caller-authored elements and Headless parts.

use crate::{ButtonVariant, Size, TOUCH_TARGET};
use octara::prelude::*;
use octara_headless::interactive;

/// Gives a control a keyboard-visible focus outline and disabled styling.
pub fn focus_outline<C>(element: El<C>) -> El<C> {
    outline(element, State::FOCUS_VISIBLE)
}

/// Gives an editor a focus outline for both keyboard and pointer focus.
pub fn input_outline<C>(element: El<C>) -> El<C> {
    outline(element, State::FOCUSED).border_color(ColorToken::Border)
}

pub(crate) fn input_group_outline<C>(element: El<C>) -> El<C> {
    outline(element, State::FOCUS_WITHIN).border_color(ColorToken::Border)
}

fn outline<C>(element: El<C>, state: State) -> El<C> {
    element
        .border(1.0)
        .style(StyleProp::ColorMotion(Spring::SNAP.into()))
        .when(state, |p| p.border_color(ColorToken::Focus).ring(2).ring_color(ColorToken::Focus.alpha(0.30)))
        .disabled_style(|p| p.opacity(0.5))
}

pub(crate) fn button_root<C>(element: El<C>) -> El<C> {
    focus_outline(interactive::button(element)).border_color(ColorToken::Border)
}

/// Applies Kit button styling and Headless button behavior to a custom element.
#[track_caller]
pub fn button_part<C>(element: El<C>, size: Size, variant: ButtonVariant) -> El<C> {
    let (height, horizontal, vertical, font) = match size {
        Size::Sm => (26.0, 2.0, 1.0, FontSizeToken::Xs),
        Size::Md => (32.0, 3.0, 1.5, FontSizeToken::Sm),
        Size::Lg => (40.0, 4.0, 2.0, FontSizeToken::Base),
    };
    let element = button_root(element)
        .px(sp(horizontal))
        .py(sp(vertical))
        .min_h(height)
        .when(When::Touch, |p| p.min_w(TOUCH_TARGET).min_h(TOUCH_TARGET).py(sp(3.0)))
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .rounded(RadiusToken::Md)
        .text_size(font)
        .text_weight(500)
        .text_color(ColorToken::Text);
    match variant {
        ButtonVariant::Primary => element
            .bg(ColorToken::Accent)
            .text_color(ColorToken::AccentText)
            .border_color(ColorToken::Accent)
            .when(State::HOVER, |p| p.bg(ColorToken::AccentHover))
            .when(State::PRESSED, |p| p.bg(ColorToken::AccentHover)),
        ButtonVariant::Secondary => element
            .bg(ColorToken::Surface)
            .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
            .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed)),
        ButtonVariant::Ghost => element
            .bg(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
            .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed)),
        ButtonVariant::Destructive => element
            .bg(ColorToken::Background)
            .text_color(ColorToken::Danger)
            .border_color(ColorToken::Danger)
            .when(State::HOVER, |p| p.bg(ColorToken::Surface))
            .when(State::PRESSED, |p| p.bg(ColorToken::SurfaceHover)),
    }
}

/// Styles the surface of an anchored popup.
pub fn popup_surface<C>(element: El<C>) -> El<C> {
    element
        .tw(tw!("p-2 gap-2 rounded-md border shadow-lg"))
        .border_color(ColorToken::Border)
        .bg(ColorToken::SurfaceElevated)
        .text_color(ColorToken::Text)
}

/// Applies the Kit input surface and focused border to an editor.
pub fn editor_style(editor: TextInput) -> TextInput {
    editor_content(input_outline(editor)).rounded(RadiusToken::Md).bg(ColorToken::Background)
}

pub(crate) fn editor_content(editor: TextInput) -> TextInput {
    editor
        .px(sp(3.0))
        .py(sp(1.5))
        .when(When::Touch, |p| p.py(sp(3.0)).min_h(TOUCH_TARGET).min_w(TOUCH_TARGET))
        .text_size(FontSizeToken::Sm)
        .text_color(ColorToken::Text)
        .bg(Color::TRANSPARENT)
}

/// Styles the scrolling container that holds a picker's options.
pub fn option_list<C>(element: El<C>) -> El<C> {
    element.gap(sp(0.5)).max_h(240).shrink(1.0).scroll_y()
}

/// Styles a picker's option, including its selected, disabled and hover states.
pub fn option_row<C>(element: El<C>) -> El<C> {
    element
        .w_full()
        .min_w(0)
        .shrink(0.0)
        .px(sp(3.0))
        .py(sp(1.5))
        .when(When::Touch, |p| p.min_h(TOUCH_TARGET))
        .rounded(RadiusToken::Sm)
        .text_size(FontSizeToken::Sm)
        .text_weight(500)
        .text_color(ColorToken::Text)
        .when(State::SELECTED, |p| p.text_color(ColorToken::Accent))
        .disabled_style(|p| p.opacity(0.5))
        .hover(|p| p.bg(ColorToken::SurfaceHover))
}
