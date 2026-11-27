//! Themed tab sets, segmented controls, and breadcrumbs.

use crate::{ButtonVariant, KitLocale, Size, parts::button_part, row_value};
use octara::prelude::*;
use octara_headless::{
    Choice, RadioGroupState, TabsState,
    tabs::{Activation, Orientation},
};
use std::{fmt::Display as KeyDisplay, hash::Hash};

#[derive(Clone, Copy, Debug, Default)]
pub struct TabsOptions {
    pub activation: Activation,
    pub orientation: Orientation,
    pub size: Size,
}

/// A tab list and retained panels over the same keyed choices.
#[ui]
#[track_caller]
pub fn tabs<K: Clone + Eq + Hash + KeyDisplay + 'static>(
    build: &mut BuildCx<'_>,
    id: &str,
    label: impl Into<String>,
    value: Binding<Option<K>>,
    options: ReadList<K, Choice>,
    appearance: TabsOptions,
    mut panel: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<Choice>) -> El + 'static,
) -> El {
    let state = TabsState::new(build, value, appearance.activation, appearance.orientation);
    let tab_id = id.to_owned();
    let panel_id = id.to_owned();
    let bar = match appearance.orientation {
        Orientation::Horizontal => row().wrap(FlexWrap::Wrap),
        Orientation::Vertical => col(),
    };
    let list = state.list(bar.test_id(id.to_owned()).label(label.into()).gap(sp(1.0))).child(each(options, move |_, key, option| {
        let tab = text(bind(move |read| row_value(option, read, |item| item.label.clone())))
            .select_none()
            .text_align(TextAlign::Center)
            .test_id(format!("{tab_id}-tab-{key}"))
            .disabled(bind(move |read| row_value(option, read, |item| item.disabled)));
        state.tab(
            button_part(tab, appearance.size, ButtonVariant::Ghost)
                .when(State::SELECTED, |style| style.bg(ColorToken::Surface).border_color(ColorToken::Accent)),
            key.clone(),
        )
    }));
    let panels = state.panels(col().grow(1.0).min_h(0.0)).child(each(options, move |build, key, option| {
        tab_panel(state, panel(build, key, option).test_id(format!("{panel_id}-panel-{key}")), key.clone())
    }));
    match appearance.orientation {
        Orientation::Horizontal => col().gap(sp(2.0)).child(list).child(panels),
        Orientation::Vertical => row().gap(sp(2.0)).child(list).child(panels),
    }
}

/// Applies the themed panel surface to a caller-authored panel within a tab set.
pub fn tab_panel<K: Clone + PartialEq + KeyDisplay + 'static>(state: TabsState<K>, element: El, key: K) -> El {
    state.panel(element.rounded(RadiusToken::Md).min_w(0.0), key)
}

/// A compact equal-width radio group.
#[ui]
#[track_caller]
pub fn segmented<K: Clone + Eq + Hash + KeyDisplay + 'static>(
    build: &mut BuildCx<'_>,
    id: &str,
    label: impl Into<String>,
    value: Binding<Option<K>>,
    options: ReadList<K, Choice>,
) -> El {
    let id = id.to_owned();
    let label = label.into();
    let state = RadioGroupState::new(build, options, value);
    state.group(
        col()
            .test_id(id.clone())
            .label(label.clone())
            .gap(sp(1.5))
            .child(text(label).test_id(format!("{id}-label")).text_size(FontSizeToken::Sm).select_none())
            .child(
                row()
                    .gap(sp(1.0))
                    .p(sp(1.0))
                    .rounded(RadiusToken::Md)
                    .bg(ColorToken::Surface)
                    .border(1.0)
                    .border_color(ColorToken::Border)
                    .child(each(options, move |_, key, option| {
                        let button = button_part(
                            text(bind(move |read| row_value(option, read, |item| item.label.clone())))
                                .select_none()
                                .text_align(TextAlign::Center),
                            Size::Md,
                            ButtonVariant::Ghost,
                        )
                        .test_id(format!("{id}-option-{key}"))
                        .description(bind(move |read| row_value(option, read, |item| item.description.clone())))
                        .grow(1.0)
                        .basis(0.0)
                        .min_w(0.0)
                        .when(State::CHECKED, |style| style.bg(ColorToken::Background).shadow(ShadowToken::Sm));
                        state.radio(button, key.clone())
                    })),
            ),
    )
}

/// Lays out caller-authored navigation links or actions with decorative separators.
#[ui]
#[track_caller]
pub fn breadcrumb(build: &mut BuildCx<'_>, crumbs: impl IntoIterator<Item = El>) -> El {
    let mut navigation = row().role(Role::Navigation).label(KitLocale::get(build).breadcrumb).gap(sp(1.0)).items_center();
    for (index, crumb) in crumbs.into_iter().enumerate() {
        if index > 0 {
            navigation =
                navigation.child(icon(octara_icons::CHEVRON_RIGHT.into()).size(sp(3.0)).text_color(ColorToken::TextMuted).a11y_hidden());
        }
        navigation = navigation.child(crumb);
    }
    navigation
}
