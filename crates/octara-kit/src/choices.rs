//! Styled choice controls backed by Headless collection and selection parts.

use crate::{
    KitLocale, PopupOptions, TOUCH_TARGET,
    parts::{editor_style, option_list, option_row, popup_surface},
    popover::apply_style,
};
use octara::prelude::*;
use octara_headless::{Choice, Collection, ComboboxState, ListboxState, Selection};
use std::{fmt::Display, hash::Hash};

/// Determines how options are mounted in the popup's scrolling list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickerOptions {
    /// Mount all options.
    Standard,
    /// Mount the visible window of options and reveal the active row as it moves.
    Virtual(VirtualOptions),
}

impl Default for PickerOptions {
    fn default() -> Self {
        Self::Virtual(VirtualOptions { estimated_row_height: TOUCH_TARGET, ..Default::default() })
    }
}

/// Rendering and styling options for searchable pickers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComboboxOptions {
    pub picker: PickerOptions,
    pub popup: PopupOptions,
    pub placeholder: Option<String>,
    pub trigger_style: Option<StylePatch>,
    pub input_style: Option<StylePatch>,
}

impl ComboboxOptions {
    pub fn picker(mut self, picker: PickerOptions) -> Self {
        self.picker = picker;
        self
    }

    pub fn popup(mut self, popup: PopupOptions) -> Self {
        self.popup = popup;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn trigger_style(mut self, style: StylePatch) -> Self {
        self.trigger_style = Some(style);
        self
    }

    pub fn input_style(mut self, style: StylePatch) -> Self {
        self.input_style = Some(style);
        self
    }

    fn editor(&self, editor: TextInput) -> TextInput {
        let editor = if let Some(placeholder) = &self.placeholder { editor.placeholder(placeholder.clone()) } else { editor };
        apply_style(editor, self.input_style.as_ref())
    }
}

/// A select with customized rows and standard or virtual rendering.
/// The trigger receives a tracked caption. The selection stores keys, allowing duplicate labels.
#[ui]
#[allow(clippy::too_many_arguments)]
pub fn select<K: Clone + Eq + Hash + Display + 'static>(
    id: &str,
    label: impl Into<String>,
    selection: Selection<K>,
    choices: ReadList<K, Choice>,
    options: PickerOptions,
    popup: PopupOptions,
    trigger: impl FnOnce(ReadSignal<String>) -> El + 'static,
    render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, Choice>) -> El + Clone + 'static,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    scope(format!("Select[{id}]"), move |b| {
        let state = ListboxState::new(b, choices, selection);
        let trigger = state.button(picker_button(b, &id, &label, selection, choices, trigger)).role(Role::ComboBox);
        let (trigger, geometry) = popup.anchor(b, trigger);
        trigger.overlay_target(
            popup.placement,
            popup.anchor_target,
            move |r| state.is_open(r),
            move |b| {
                let list = picker_list(&id, &label, popup.max_height.is_some());
                let render = styled_options(&id, state.collection(), render.clone());
                let list = match options {
                    PickerOptions::Standard => state.options(list, render),
                    PickerOptions::Virtual(options) => state.virtual_options(b, list, options, render),
                };
                popup.surface(popup_surface(col()).test_id(format!("{id}-popover")).min_w(160), geometry).child(list)
            },
        )
    })
}

/// A searchable picker with single or multiple selection, custom rows, and optional virtualization.
#[ui]
pub fn combobox<K: Clone + Eq + Hash + Display + 'static>(
    id: &str,
    label: impl Into<String>,
    selection: Selection<K>,
    choices: ReadList<K, Choice>,
    options: ComboboxOptions,
    trigger: impl FnOnce(ReadSignal<String>) -> El + 'static,
    render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, Choice>) -> El + Clone + 'static,
) -> View {
    let id = id.to_owned();
    let label = label.into();
    scope(format!("Combobox[{id}]"), move |b| {
        let state = ComboboxState::new(b, choices, selection);
        let trigger = apply_style(picker_button(b, &id, &label, selection, choices, trigger), options.trigger_style.as_ref());
        let (trigger, geometry) = options.popup.anchor(b, trigger);
        state.anchored_overlay(trigger, options.popup.placement, options.popup.anchor_target, move |b, state| {
            let editor =
                editor_style(input()).test_id(format!("{id}-search")).label(KitLocale::get(b).search_label(&label)).w_full().autofocus();
            options
                .popup
                .surface(popup_surface(col()).test_id(format!("{id}-popover")).w(260), geometry)
                .child(state.input(options.editor(editor)))
                .child(combobox_options(b, &id, &label, state, options.picker, options.popup.max_height.is_some(), render.clone()))
        })
    })
}

/// Styles an editable combobox with suggestions anchored to a caller-authored editor.
/// The state owns the query and selection. The editor retains its focus and application handlers.
/// Name the editor on the supplied element and pass the popup list's name as `list_label`.
#[ui]
pub fn combobox_input<K: Clone + Eq + Hash + Display + 'static, T: 'static>(
    b: &mut BuildCx<'_>,
    id: &str,
    list_label: impl Into<String>,
    state: ComboboxState<K, T>,
    editor: TextInput,
    options: ComboboxOptions,
    render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + Clone + 'static,
) -> TextInput {
    let id = id.to_owned();
    let label = list_label.into();
    let editor = apply_style(editor_style(editor).test_id(id.clone()), options.trigger_style.as_ref());
    let (editor, geometry) = options.popup.anchor(b, state.input(options.editor(editor)));
    editor.overlay_target(
        options.popup.placement,
        options.popup.anchor_target,
        move |r| state.is_open(r),
        move |b| {
            options.popup.surface(popup_surface(col()).test_id(format!("{id}-popover")).min_w(160), geometry).child(combobox_options(
                b,
                &id,
                &label,
                state,
                options.picker,
                options.popup.max_height.is_some(),
                render.clone(),
            ))
        },
    )
}

fn picker_button<K: Clone + Eq + Hash + 'static>(
    b: &mut BuildCx<'_>,
    id: &str,
    label: &str,
    selection: Selection<K>,
    choices: ReadList<K, Choice>,
    trigger: impl FnOnce(ReadSignal<String>) -> El,
) -> El {
    let fallback = label.to_owned();
    let caption = b.memo(move |r| {
        let labels = selection
            .keys(r)
            .into_iter()
            .filter_map(|key| r.with_row(choices, &key, |choice| choice.map(|choice| choice.label.clone())))
            .collect::<Vec<_>>();
        if labels.is_empty() { fallback.clone() } else { labels.join(", ") }
    });
    trigger(caption).test_id(id.to_owned()).label(label.to_owned()).a11y_value(caption)
}

fn picker_list(id: &str, label: &str, surface_constrained: bool) -> El {
    let list = option_list(col().test_id(format!("{id}-list")).label(label.to_owned()));
    if surface_constrained { list.max_h(Length::Auto) } else { list }
}

pub(crate) fn combobox_options<K: Clone + Eq + Hash + Display + 'static, T: 'static>(
    b: &mut BuildCx<'_>,
    id: &str,
    label: &str,
    state: ComboboxState<K, T>,
    options: PickerOptions,
    surface_constrained: bool,
    render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
) -> El {
    let collection = state.collection();
    let list = picker_list(id, label, surface_constrained);
    let render = styled_options(id, collection, render);
    let list = match options {
        PickerOptions::Standard => state.options(list, render),
        PickerOptions::Virtual(options) => state.virtual_options(b, list, options, render),
    };
    let empty_id = format!("{id}-empty");
    let no_results = KitLocale::get(b).no_results;
    col().shrink(1.0).min_h(0).child(list).child(show(
        move |r| r.len(collection.rows()) == 0,
        move |_| {
            text(no_results)
                .test_id(empty_id.clone())
                .px(sp(3.0))
                .py(sp(2.0))
                .text_size(FontSizeToken::Sm)
                .text_color(ColorToken::TextMuted)
        },
    ))
}

/// Renders the label of a Headless choice as the popup's default text row.
pub fn choice_row<K: Eq + Hash + 'static>(_: &mut BuildCx<'_>, _: &K, choice: RowSignal<K, Choice>) -> El {
    text(move |r: &mut ReadCx<'_>| choice.with(r, |choice| choice.map(|choice| choice.label.clone()).unwrap_or_default()))
        .text_overflow(TextOverflow::Ellipsis)
}

fn styled_options<K: Clone + Eq + Hash + Display + 'static, T: 'static>(
    id: &str,
    collection: Collection<K, T>,
    mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
) -> impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static {
    let id = id.to_owned();
    move |b, key, row| {
        let option = render(b, key, row).test_id(format!("{id}-option-{key}"));
        let key = key.clone();
        option_row(option)
            .style_with(move |r, p| if collection.active(r).as_ref() == Some(&key) { p.bg(ColorToken::SurfacePressed) } else { p })
    }
}
