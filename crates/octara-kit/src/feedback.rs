//! Toast notifications and a searchable modal command palette.

use crate::{
    DialogOptions, PickerOptions,
    choices::{choice_row, combobox_options},
    dialog, icon_button,
    parts::{editor_style, popup_surface},
};
use octara::prelude::*;
use octara_headless::{Choice, ComboboxState, Selection, ToastQueue};

/// Wraps a vertical scrollport with a themed pull indicator and refresh spinner.
#[ui]
pub fn pull_to_refresh(state: crate::PullToRefresh, scrollport: El, refresh: impl Fn(&mut WriteCx<'_>) + 'static) -> El {
    col().position(Position::Relative).min_h(0).child((
        state.attach(scrollport, refresh),
        show(
            move |read| state.distance(read) > 0.5 || state.is_refreshing(read),
            move |_| {
                row()
                    .position(Position::Absolute)
                    .left(0)
                    .right(0)
                    .justify(JustifyContent::Center)
                    .pointer_events_none()
                    .style_with(move |read, style| {
                        let top = if state.is_refreshing(read) { 8.0 } else { (state.distance(read) - 28.0).clamp(0.0, 24.0) };
                        style.top(f64::from(top))
                    })
                    .child(
                        popup_surface(row())
                            .size(32)
                            .align(AlignItems::Center)
                            .justify(JustifyContent::Center)
                            .pointer_events_none()
                            .child((
                                show(move |read| state.is_refreshing(read), |build| crate::spinner(build).pointer_events_none()),
                                show(
                                    move |read| !state.is_refreshing(read),
                                    move |_| {
                                        text("↓")
                                            .pointer_events_none()
                                            .opacity(move |read: &mut ReadCx<'_>| (state.distance(read) / state.threshold()).min(1.0))
                                    },
                                ),
                            )),
                    )
            },
        ),
    ))
}

/// Mounts queued toasts in the bottom-right corner of the surrounding view.
/// Timers pause when the pointer rests on a toast.
#[ui]
pub fn toast_viewport(id: &str, queue: ToastQueue<String>) -> El {
    let id = id.to_owned();
    col().test_id(id.clone()).position(Position::Absolute).right(sp(4.0)).bottom(sp(4.0)).w(320).max_w(rel(0.85)).gap(sp(2.0)).child(each(
        queue.items(),
        move |b, key, toast| {
            let key = *key;
            let locale = crate::KitLocale::get(b);
            let message = text(move |r: &mut ReadCx<'_>| r.try_with(toast, |toast| toast.value.clone()).unwrap_or_default())
                .min_w(0)
                .grow(1.0)
                .text_size(FontSizeToken::Sm);
            popup_surface(row())
                .test_id(format!("{id}-{key}"))
                .align(AlignItems::Center)
                .on_hover_change(move |w, hovered| {
                    if hovered {
                        queue.pause(w, key);
                    } else {
                        queue.resume(w, key);
                    }
                })
                .child(queue.item(b, message, key))
                .child(icon_button(octara_icons::X, locale.dismiss_notification).test_id(format!("{id}-{key}-dismiss")).on_press(
                    move |w| {
                        queue.dismiss(w, key);
                    },
                ))
        },
    ))
}

/// Opens a modal command picker, then closes it and supplies the chosen key.
/// Choice filtering and keyboard interaction use the same Headless combobox as other pickers.
#[ui]
pub fn command_palette(
    id: &str,
    title: impl Into<String>,
    open: Binding<bool>,
    commands: ReadList<String, Choice>,
    on_choose: impl Fn(&mut WriteCx<'_>, String) + Clone + 'static,
) -> View {
    let palette = id.to_owned();
    dialog(id, title, open, DialogOptions::default(), move |b| {
        let locale = crate::KitLocale::get(b);
        let on_choose = on_choose.clone();
        let selected = b.binding(
            |_| None,
            move |w, key| {
                if let Some(key) = key {
                    w.set(open, false);
                    on_choose(w, key);
                }
            },
        );
        let picker = ComboboxState::new(b, commands, Selection::Single(selected)).within_layer();
        b.watch(
            move |r| !picker.collection().is_empty(r),
            move |w, available, _| {
                if *available {
                    picker.open(w);
                }
            },
        );
        let id = palette.clone();
        col()
            .gap(sp(2.0))
            .child(
                picker.input(
                    editor_style(input())
                        .test_id(format!("{id}-search"))
                        .label(locale.search_commands)
                        .placeholder(locale.search_commands)
                        .autofocus(),
                ),
            )
            .child(show(
                move |r| picker.is_open(r),
                move |b| combobox_options(b, &id, locale.commands, picker, PickerOptions::default(), false, choice_row),
            ))
    })
}
