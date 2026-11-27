//! An editable picker that keeps real focus in its input.

use std::{fmt::Display, hash::Hash};

use octara::prelude::*;

use crate::{
    Control,
    collection::{Choice, Collection, Navigation, Selection},
    listbox::Picker,
};

/// An editable combobox with single or multiple selection and separate query text.
///
/// Each key must have a unique display form for option IDs and virtual reveal.
pub struct ComboboxState<K, T = Choice> {
    picker: Picker<K, T>,
    query: Binding<String>,
    composing: Signal<bool>,
    presentation: Presentation,
}

#[derive(Clone, Copy)]
enum Presentation {
    Editable,
    Anchored,
    WithinLayer,
}

impl<K, T> Copy for ComboboxState<K, T> {}

impl<K, T> Clone for ComboboxState<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + Display + 'static> ComboboxState<K> {
    /// Filters choices by a case-insensitive substring of their labels.
    pub fn new(b: &mut BuildCx<'_>, rows: impl IntoReadList<K, Choice>, selection: Selection<K>) -> Self {
        Self::with_query(b, rows, selection, Control::Uncontrolled(String::new()))
    }

    /// Uses a caller-owned query with the default choice filtering.
    pub fn with_query(
        b: &mut BuildCx<'_>,
        rows: impl IntoReadList<K, Choice>,
        selection: Selection<K>,
        query: impl Into<Control<String>>,
    ) -> Self {
        let rows = rows.into_read_list();
        let query = query.into().bind(b);
        let enabled = rows.filter(b, |row| !row.disabled);
        let available = b.memo(move |r| r.len(enabled) > 0);
        let normalized = b.memo(move |r| r.with(query, |query| query.to_lowercase()));
        let visible = rows.filter_with(b, normalized, |row, query| query.is_empty() || row.label.to_lowercase().contains(query));
        let collection = Collection::choices(b, visible);
        Self::with_collection(b, collection, selection, query, available)
    }
}

impl<K: Clone + Eq + Hash + Display + 'static, T: 'static> ComboboxState<K, T> {
    /// Uses a custom collection, query and availability predicate.
    ///
    /// `available` controls whether the input and button can open the picker and can
    /// remain true when a filter returns no results.
    pub fn with_collection(
        b: &mut BuildCx<'_>,
        collection: Collection<K, T>,
        selection: Selection<K>,
        query: impl Into<Control<String>>,
        available: ReadSignal<bool>,
    ) -> Self {
        let query = query.into().bind(b);
        let state = Self {
            picker: Picker::new(b, collection, selection, available),
            query,
            composing: b.signal(false),
            presentation: Presentation::Editable,
        };
        b.watch(
            move |r| (r.get(query), state.is_open(r)),
            move |w, (query, open), previous| {
                if *open && previous.is_some_and(|(old_query, was_open)| *was_open && old_query != query) {
                    state.picker.reset_active(w);
                }
            },
        );
        state
    }

    pub fn collection(self) -> Collection<K, T> {
        self.picker.collection
    }

    pub fn selection(self) -> Selection<K> {
        self.picker.selection
    }

    pub fn query(self) -> Binding<String> {
        self.query
    }

    pub fn is_open(self, r: &mut ReadCx<'_>) -> bool {
        self.picker.is_open(r)
    }

    pub fn open(self, w: &mut WriteCx<'_>) {
        if matches!(self.presentation, Presentation::Anchored) && w.get(self.picker.open).is_none() {
            w.set_if_changed(self.composing, false);
            if w.with(self.query, |query| !query.is_empty()) {
                w.set(self.query, String::new());
            }
        }
        self.picker.open(w, Navigation::First);
    }

    pub fn close(self, w: &mut WriteCx<'_>) {
        self.picker.close(w);
    }

    pub fn input(self, el: TextInput) -> TextInput {
        // The focused input can receive option commands even when the list is portaled.
        self.picker
            .option_commands(el)
            .id(self.input_id())
            .role(Role::EditableComboBox)
            .disabled_when(bind(move |r| !r.get(self.picker.available)))
            .layer_group(self.picker.group)
            .expanded(bind(move |r| self.is_open(r)))
            .controls(bind(move |r| self.picker.controls(r)))
            .active_descendant(bind(move |r| self.picker.active_descendant(r)))
            .observe_composition(self.composing)
            .value(self.query)
            .behavior_first(|behavior| {
                behavior
                    .on_input(move |w, text| {
                        w.set(self.query, text.to_owned());
                        self.open(w);
                    })
                    .on_key(move |w, event| self.input_key(w, event))
            })
    }

    pub fn button<C>(self, el: El<C>) -> El<C> {
        self.picker.button(el).preserve_focus_on_press().behavior_first(|behavior| {
            behavior.on_press(move |w| {
                if w.get(self.picker.open).is_some() {
                    self.close(w);
                } else {
                    self.open(w);
                    w.focus(&self.input_id());
                }
            })
        })
    }

    /// Attaches a button that opens an anchored popup containing both the input and options.
    /// Each opening clears the query. Build both parts with the state supplied to the content factory.
    pub fn anchored_overlay<C>(
        self,
        button: El<C>,
        placement: Placement,
        target: Option<AnchorTarget>,
        mut content: impl FnMut(&mut BuildCx<'_>, Self) -> El + 'static,
    ) -> El<C> {
        let state = Self { presentation: Presentation::Anchored, ..self };
        let policy =
            LayerPolicy { focus_trap: FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::RESTORE_FOCUS, ..LayerPolicy::non_modal() };
        state.button(button).role(Role::ComboBox).overlay_target(
            placement,
            target,
            move |r| state.is_open(r),
            move |b| state.picker.layer(content(b, state), policy),
        )
    }

    /// Renders options within an existing layer, such as a command palette dialog.
    /// The parent layer owns Escape and outside dismissal.
    pub fn within_layer(self) -> Self {
        Self { presentation: Presentation::WithinLayer, ..self }
    }

    /// Configures the list element. Mount it in an overlay or a conditional region while open.
    pub fn options_part<C>(self, el: El<C>) -> El<C> {
        let el = self.picker.options_part(el);
        match self.presentation {
            Presentation::Editable => self.picker.layer(el, LayerPolicy::non_modal()),
            Presentation::Anchored | Presentation::WithinLayer => el,
        }
    }

    /// Configures the list and builds a caller-provided element for each option.
    pub fn options(self, el: El, render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<T>) -> El + 'static) -> El {
        self.picker.render_options(self.options_part(el), render, None, Some(self.composing))
    }

    /// Configures a virtualized list. The supplied element must be the scroll container.
    ///
    /// Declare disabled state in the collection so navigation can skip disabled rows
    /// while they are outside the mounted viewport.
    pub fn virtual_options(
        self,
        b: &mut BuildCx<'_>,
        el: El,
        options: VirtualOptions,
        render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<T>) -> El + 'static,
    ) -> El {
        let geometry = self.picker.observe_virtual(b);
        self.picker.render_options(self.options_part(el).observe_geometry(geometry), render, Some(options), Some(self.composing))
    }

    pub fn option<C>(self, el: El<C>, key: K) -> El<C> {
        self.picker.option(el, key, Some(self.composing))
    }

    fn input_id(self) -> String {
        self.picker.id.part("input")
    }

    fn input_key(self, w: &mut WriteCx<'_>, event: &KeyEvent) -> bool {
        if w.get(self.composing) || !event.modifiers.is_empty() {
            return false;
        }
        let open = w.get(self.picker.open).is_some();
        match event.key {
            Key::Enter if open => {
                if !event.repeat {
                    self.picker.commit_active(w);
                }
            }
            Key::Up | Key::Down => {
                if open {
                    let movement = if event.key == Key::Up { Navigation::Previous } else { Navigation::Next };
                    self.picker.navigate(w, movement);
                } else {
                    let fallback = if event.key == Key::Up { Navigation::Last } else { Navigation::First };
                    self.picker.open(w, fallback);
                }
            }
            _ => return false,
        }
        true
    }
}
