//! Structure: a section that opens and closes (collapsible), lists of rows (list, list_item,
//! list_header), and two panes with a divider between them (split). Open, selection and width
//! are controlled by the caller.

use octara::prelude::*;
use octara_icons as icons;

/// A header that opens and closes its content below (SwiftUI's `DisclosureGroup`): a reasoning
/// trace, a tool call's detail, a settings group. `open` holds whether it is open; `content`
/// returns any [`IntoView`] and is built in the open section's scope. `id` names it; its trigger is
/// `{id}-trigger` (a `Button` with `expanded`), its content `{id}-content`. The trigger's
/// `controls` relation tracks the mounted content.
#[ui]
#[track_caller]
pub fn collapsible<V: IntoView + 'static>(
    build: &mut BuildCx<'_>,
    id: &str,
    open: Binding<bool>,
    header: impl IntoView,
    mut content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> El {
    let disclosure = octara_headless::DisclosureState::new(build, open);
    let content_id = format!("{id}-content");
    let trigger = crate::parts::button_part(
        row()
            .test_id(format!("{id}-trigger"))
            .align(AlignItems::Center)
            .gap(sp(1.5))
            .child(keyed(
                move |read| disclosure.is_open(read),
                |_, expanded| {
                    let glyph = if *expanded { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                    icon(glyph.into()).size(sp(3.5)).shrink(0.0).a11y_hidden()
                },
            ))
            .child(header),
        crate::Size::Sm,
        crate::ButtonVariant::Ghost,
    );
    col()
        .test_id(id.to_owned())
        .role(Role::Group)
        .child(disclosure.button(trigger))
        .child(disclosure.panel(move |build| col().test_id(content_id.clone()).child(content(build))))
}

/// A `ListBox` of `list_item`s with optional `list_header`s between groups. Up and Down move
/// focus between enabled items and stop at the ends. Home and End focus the first and last
/// enabled items. Make it scroll with `overflow_y(Overflow::Scroll)`, and give it `each` or `virtual_each`
/// children.
#[ui]
#[track_caller]
pub fn list() -> El {
    col().role(Role::ListBox).focus_group(FocusGroup::vertical()).gap(sp(0.5))
}

/// A row of a `list` showing `content`: hovered, focused and selected looks. Chain
/// `.selected(..)`, `.on_press(..)` (select it), `.on(HandlerKind::SecondaryPress, ..)` (its context menu).
/// Use `.focus_preferred(..)` to mark a preferred Tab entry for the list.
#[ui]
#[track_caller]
pub fn list_item(content: impl IntoView) -> El {
    row()
        .role(Role::ListBoxOption)
        .focusable()
        .align(AlignItems::Center)
        .gap(sp(2.0))
        .min_h(32.0)
        .when(When::Touch, |p| p.min_h(44.0))
        .px(sp(2.0))
        .rounded(RadiusToken::Md)
        .border(1.0)
        .border_color(ColorValue::Color(Color::TRANSPARENT))
        .text_size(FontSizeToken::Sm)
        .text_color(ColorToken::Text)
        .style(StyleProp::ColorMotion(Spring::SNAP.into()))
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::SELECTED, |p| p.bg(ColorToken::SurfacePressed))
        .when(State::FOCUS_VISIBLE, |p| p.border_color(ColorToken::Focus))
        .child(content)
}

/// The heading of a group of rows in a `list` ("Today", "Yesterday").
#[ui]
#[track_caller]
pub fn list_header(label: impl IntoText) -> El {
    text(label)
        .role(Role::Heading)
        .level(3)
        .px(sp(2.0))
        .pt(sp(3.0))
        .pb(sp(1.0))
        .text_size(FontSizeToken::Xs)
        .text_weight(500)
        .text_color(ColorToken::TextMuted)
        .select_none()
}

/// The narrowest a `split`'s leading pane is dragged to.
const MIN_PANE: f32 = 120.0;

/// Two panes side by side (AppKit's `NSSplitView`): `leading` is `width` wide, `trailing` takes
/// the rest; dragging the divider between them, or its arrow keys when focused, changes `width`
/// (at least 120; the setter may clamp it more). `id` names it; its divider is `{id}-divider`, a
/// `Splitter` whose value is the width: a line in a strip 7 wide to grab (20 on touch).
#[ui]
#[track_caller]
pub fn split(build: &mut BuildCx<'_>, id: &str, width: Binding<f32>, leading: impl IntoView, trailing: impl IntoView) -> El {
    let bounded = build.binding(move |r| width.get(r), move |w, size: f32| width.set(w, size.max(MIN_PANE)));
    let handle = splitter(build, &format!("{id}-divider"), bounded);
    row()
        .test_id(id.to_owned())
        .size(rel(1.0))
        .child(el().h(rel(1.0)).shrink(0.0).style_with(move |r, s| s.w(width.get(r))).child(leading))
        .child(handle)
        .child(el().h(rel(1.0)).grow(1.0).basis(0.0).min_w(0.0).child(trailing))
}

/// A horizontal resize handle, independent of pane layout. `id` names the handle itself.
/// Dragging proposes the width at drag start plus total horizontal movement; Left/Right
/// propose changes of 8, or 40 with Shift. The model owns bounds and may reject or normalize
/// writes; displayed values always read the accepted width. This component imposes no bounds.
/// Add application handlers without replacing resize behavior. Use it where a responsive
/// layout must mount its handle separately from its panes.
#[ui]
#[track_caller]
pub fn splitter(build: &mut BuildCx<'_>, id: &str, width: Binding<f32>) -> El {
    octara_headless::splitter::SplitterState::new(build, width).handle(
        el().test_id(id.to_owned())
            .label(crate::KitLocale::get(build).resize)
            .w(7.0)
            .when(When::Touch, |style| style.min_w(crate::TOUCH_TARGET).min_h(crate::TOUCH_TARGET))
            .h(rel(1.0))
            .shrink(0.0)
            .justify(JustifyContent::Center)
            .cursor(CursorStyle::ResizeLeftRight)
            .child(
                el().w(1.0)
                    .h(rel(1.0))
                    .bg(ColorToken::Border)
                    .when(State::FOCUS_VISIBLE, |style| style.bg(ColorToken::Focus))
                    .a11y_hidden(),
            ),
    )
}

/// A horizontal rule with separator semantics.
#[ui]
#[track_caller]
pub fn separator() -> El {
    el().role(Role::Separator).w(rel(1.0)).h(1.0).shrink(0.0).bg(ColorToken::Border)
}
