//! Structure: a section that opens and closes (collapsible), lists of rows (list, list_item,
//! list_header), and two panes with a divider between them (split). They keep no state of their
//! own: what is open, selected or how wide comes from the caller.

use crate::Model;
use octara::prelude::*;
use octara_icons as icons;
use std::cell::Cell;
use std::rc::Rc;

/// A header that opens and closes its content below (SwiftUI's `DisclosureGroup`): a reasoning
/// trace, a tool call's detail, a settings group. `open` holds whether it is open; `content`
/// is built while it is (a scope: its state lives as long). `id` names it; its trigger is
/// `{id}-trigger` (a `Button` with `expanded`), its content `{id}-content`.
#[component]
#[track_caller]
pub fn collapsible(id: &str, open: impl Model<bool>, header: impl IntoElement, content: impl Fn() -> View + 'static) -> El {
    let open = Rc::new(open);
    let (shown, flip, expanded, chevron) = (open.clone(), open.clone(), open.clone(), open.clone());
    let content_id = format!("{id}-content");
    let trigger = row()
        .id(format!("{id}-trigger"))
        .role(Role::Button)
        .focusable()
        .expanded(move || expanded.get())
        .on_press(move || flip.set(!flip.get()))
        .items_center()
        .gap_1p5()
        .py_1()
        .px_1()
        .rounded_md()
        .border()
        .border_color(Paint::Color(Color::TRANSPARENT))
        .focus_visible(|p| p.border_color(Token::Focus))
        .text_sm()
        .text_color(Token::TextMuted)
        .hover(|p| p.text_color(Token::Text))
        .color_motion(Spring::SNAP)
        .child(show_else(
            move || chevron.get(),
            || icon(icons::CHEVRON_DOWN).size_3p5().flex_shrink_0().a11y_hidden().into(),
            || icon(icons::CHEVRON_RIGHT).size_3p5().flex_shrink_0().a11y_hidden().into(),
        ))
        .child(header);
    col()
        .id(id.to_owned())
        .role(Role::Group)
        .child(trigger)
        .child(show(move || shown.get(), move || col().id(content_id.clone()).child(content()).into()))
}

/// A list of rows (`list_item`s, `list_header`s between groups): a `ListBox` whose arrow keys
/// move between its items. Make it scroll with `overflow_y_scroll`, give it `each` or
/// `virtual_each` children.
#[component]
#[track_caller]
pub fn list() -> El {
    col().role(Role::ListBox).focus_group().gap_0p5()
}

/// A row of a `list` showing `content`: hovered, focused and selected looks. Chain
/// `.selected(..)`, `.on_press(..)` (select it), `.on_secondary_press(..)` (its context menu).
#[component]
#[track_caller]
pub fn list_item(content: impl IntoElement) -> El {
    row()
        .role(Role::ListBoxOption)
        .focusable()
        .items_center()
        .gap_2()
        .min_h(32.0)
        .when_touch(|p| p.min_h(44.0))
        .px_2()
        .rounded_md()
        .border()
        .border_color(Paint::Color(Color::TRANSPARENT))
        .text_sm()
        .text_color(Token::Text)
        .color_motion(Spring::SNAP)
        .hover(|p| p.bg(Token::SurfaceHover))
        .style_when(State::SELECTED, |p| p.bg(Token::SurfacePressed))
        .focus_visible(|p| p.border_color(Token::Focus))
        .child(content)
}

/// The heading of a group of rows in a `list` ("Today", "Yesterday").
#[component]
#[track_caller]
pub fn list_header(label: impl IntoText) -> El {
    text(label).role(Role::Heading).level(3).px_2().pt_3().pb_1().text_xs().font_medium().text_color(Token::TextMuted).select_none()
}

/// The narrowest a `split`'s leading pane is dragged to.
const MIN_PANE: f32 = 120.0;

/// Two panes side by side (AppKit's `NSSplitView`): `leading` is `width` wide, `trailing` takes
/// the rest; dragging the divider between them, or its arrow keys when focused, changes `width`
/// (at least 120; the setter may clamp it more). `id` names it; its divider is `{id}-divider`, a
/// `Splitter` whose value is the width: a line in a strip 7 wide to grab (20 on touch).
#[component]
#[track_caller]
pub fn split(id: &str, width: impl Model<f32>, leading: impl IntoElement, trailing: impl IntoElement) -> El {
    let width = Rc::new(width);
    let start = Rc::new(Cell::new(0.0f32));
    let (read, drag, keys, value) = (width.clone(), width.clone(), width.clone(), width);
    let handle = el()
        .id(format!("{id}-divider"))
        .role(Role::Splitter)
        .label("Resize")
        .value(move || format!("{}", value.get().round()))
        .focusable()
        .w(7.0)
        .when_touch(|p| p.w(20.0))
        .h_full()
        .flex_shrink_0()
        .justify_center()
        .cursor(CursorStyle::ResizeLeftRight)
        .child(el().w(1.0).h_full().bg(Token::Border).style_when(State::FOCUS_VISIBLE, |p| p.bg(Token::Focus)).a11y_hidden())
        .on_drag(move |d| match d.phase {
            DragPhase::Start => start.set(drag.get()),
            DragPhase::Move => drag.set((start.get() + d.dx).max(MIN_PANE)),
            _ => {}
        })
        .on_key(move |k| {
            let step = if k.modifiers.contains(Modifiers::SHIFT) { 40.0 } else { 8.0 };
            let by = match k.key {
                Key::Left => -step,
                Key::Right => step,
                _ => return false,
            };
            keys.set((keys.get() + by).max(MIN_PANE));
            true
        });
    row()
        .id(id.to_owned())
        .size_full()
        .child(el().h_full().flex_shrink_0().style_with(move |s| s.w(read.get())).child(leading))
        .child(handle)
        .child(el().h_full().flex_1().min_w(0.0).child(trailing))
}
