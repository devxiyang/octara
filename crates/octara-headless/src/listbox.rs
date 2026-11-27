//! A picker that keeps keyboard focus on its list of options.

use std::{fmt::Display, hash::Hash};

use octara::prelude::*;

use crate::{
    collection::{Choice, Collection, Navigation, Selection},
    id::Id,
};

/// A single- or multiple-selection listbox with keyed options.
///
/// A key's display form must be unique within the collection because option IDs
/// and virtual reveal use it. Labels may repeat.
pub struct ListboxState<K, T = Choice>(Picker<K, T>);

impl<K, T> Copy for ListboxState<K, T> {}

impl<K, T> Clone for ListboxState<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + Display + 'static> ListboxState<K> {
    pub fn new(b: &mut BuildCx<'_>, rows: impl IntoReadList<K, Choice>, selection: Selection<K>) -> Self {
        let collection = Collection::choices(b, rows);
        Self::with_collection(b, collection, selection)
    }
}

impl<K: Clone + Eq + Hash + Display + 'static, T: 'static> ListboxState<K, T> {
    pub fn with_collection(b: &mut BuildCx<'_>, collection: Collection<K, T>, selection: Selection<K>) -> Self {
        let available = b.memo(move |r| !collection.is_empty(r));
        Self(Picker::new(b, collection, selection, available))
    }

    pub fn collection(self) -> Collection<K, T> {
        self.0.collection
    }

    pub fn selection(self) -> Selection<K> {
        self.0.selection
    }

    pub fn is_open(self, r: &mut ReadCx<'_>) -> bool {
        self.0.is_open(r)
    }

    pub fn open(self, w: &mut WriteCx<'_>) {
        self.0.open(w, Navigation::First);
    }

    pub fn close(self, w: &mut WriteCx<'_>) {
        self.0.close(w);
    }

    pub fn button<C>(self, el: El<C>) -> El<C> {
        self.0.button(el).behavior_first(|behavior| {
            behavior
                .on_press(move |w| {
                    if w.get(self.0.open).is_some() {
                        self.close(w);
                    } else {
                        self.open(w);
                    }
                })
                .on_key(move |w, event| {
                    if !event.modifiers.is_empty() {
                        return false;
                    }
                    let fallback = match event.key {
                        Key::Down => Navigation::First,
                        Key::Up => Navigation::Last,
                        _ => return false,
                    };
                    self.0.open(w, fallback);
                    w.focus(&self.0.options_id());
                    true
                })
        })
    }

    /// Configures the list element. Mount it in an overlay or a conditional region while open.
    pub fn options_part<C>(self, el: El<C>) -> El<C> {
        let mut policy = LayerPolicy::non_modal();
        policy.focus_trap = FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::RESTORE_FOCUS;
        self.0
            .layer(self.0.options_part(el), policy)
            .focusable()
            .active_descendant(bind(move |r| self.0.active_descendant(r)))
            .behavior_first(|behavior| behavior.on_key(move |w, event| self.options_key(w, event)))
    }

    /// Configures the list and builds a caller-provided element for each option.
    pub fn options(self, el: El, render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static) -> El {
        self.0.render_options(self.options_part(el), render, None, None)
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
        render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
    ) -> El {
        let geometry = self.0.observe_virtual(b);
        self.0.render_options(self.options_part(el).observe_geometry(geometry), render, Some(options), None)
    }

    pub fn option<C>(self, el: El<C>, key: K) -> El<C> {
        self.0.option(el, key, None)
    }

    fn options_key(self, w: &mut WriteCx<'_>, event: &KeyEvent) -> bool {
        if let Key::Char(character) = event.key
            && (event.modifiers - Modifiers::SHIFT).is_empty()
        {
            self.0.collection.typeahead_when(w, character, |w, key| self.0.can_activate(w, key));
            return true;
        }
        if !event.modifiers.is_empty() {
            return false;
        }
        let navigation = match event.key {
            Key::Up => Navigation::Previous,
            Key::Down => Navigation::Next,
            Key::Home => Navigation::First,
            Key::End => Navigation::Last,
            Key::Enter | Key::Space => {
                if !event.repeat {
                    self.0.commit_active(w);
                }
                return true;
            }
            _ => return false,
        };
        self.0.navigate(w, navigation);
        true
    }
}

pub(crate) struct Picker<K, T> {
    pub(crate) collection: Collection<K, T>,
    pub(crate) selection: Selection<K>,
    pub(crate) open: Signal<Option<Navigation>>,
    pub(crate) available: ReadSignal<bool>,
    pub(crate) group: LayerGroupId,
    pub(crate) id: Id,
}

impl<K, T> Copy for Picker<K, T> {}

impl<K, T> Clone for Picker<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + Display + 'static, T: 'static> Picker<K, T> {
    pub(crate) fn new(b: &mut BuildCx<'_>, collection: Collection<K, T>, selection: Selection<K>, available: ReadSignal<bool>) -> Self {
        let picker = Self { collection, selection, available, open: b.signal(None), group: LayerGroupId::new(), id: Id::new() };
        b.watch(
            move |r| r.get(available),
            move |w, available, _| {
                if !available {
                    picker.close(w);
                }
            },
        );
        picker
    }

    pub(crate) fn options_id(self) -> String {
        self.id.part("options")
    }

    fn option_id(self, key: &K) -> String {
        self.id.part(&format!("option-{key}"))
    }

    pub(crate) fn is_open(self, r: &mut ReadCx<'_>) -> bool {
        r.get(self.open).is_some()
    }

    pub(crate) fn controls(self, r: &mut ReadCx<'_>) -> Option<String> {
        self.is_open(r).then(|| self.options_id())
    }

    pub(crate) fn active_descendant(self, r: &mut ReadCx<'_>) -> Option<String> {
        // The facade's semantic relation resolves only to a mounted, accessible option.
        self.is_open(r).then(|| self.collection.active(r)).flatten().map(|key| self.option_id(&key))
    }

    pub(crate) fn button<C>(self, el: El<C>) -> El<C> {
        el.id(self.id.part("button"))
            .role(Role::Button)
            .focusable()
            .disabled_when(bind(move |r| !r.get(self.available)))
            .expanded(bind(move |r| self.is_open(r)))
            .controls(bind(move |r| self.controls(r)))
            .layer_group(self.group)
    }

    pub(crate) fn options_part<C>(self, el: El<C>) -> El<C> {
        self.option_commands(
            el.id(self.options_id()).role(Role::ListBox).multi_selectable(self.selection.is_multiple()).preserve_focus_on_press(),
        )
    }

    pub(crate) fn layer<C>(self, el: El<C>, mut policy: LayerPolicy) -> El<C> {
        policy.group = Some(self.group);
        el.layer(policy, move |w| self.close(w))
    }

    pub(crate) fn option_commands<C>(self, el: El<C>) -> El<C> {
        el.on_command(self.id.part("check-initial-option"), move |w| {
            if let Some(fallback) = w.get(self.open)
                && self.active_is_disabled(w)
            {
                self.activate_initial(w, fallback);
            }
        })
        .on_command(self.id.part("reset-active-option"), move |w| {
            if w.get(self.open).is_some() && self.active_is_disabled(w) {
                self.navigate(w, Navigation::First);
            }
        })
    }

    pub(crate) fn observe_virtual(self, b: &mut BuildCx<'_>) -> Signal<Option<LayoutBox>> {
        let geometry = b.signal(None);
        self.collection.observe_virtual(b, self.options_id(), geometry);
        geometry
    }

    pub(crate) fn render_options(
        self,
        el: El,
        mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
        options: Option<VirtualOptions>,
        composing: Option<Signal<bool>>,
    ) -> El {
        let render = move |b: &mut BuildCx<'_>, key: &K, row| self.option(render(b, key, row), key.clone(), composing);
        let rows = self.collection.rows();
        el.child(match options {
            Some(options) => virtual_each(rows, options, render),
            None => each(rows, render),
        })
    }

    pub(crate) fn option<C>(self, el: El<C>, key: K, composing: Option<Signal<bool>>) -> El<C> {
        let disabled = key.clone();
        let selected = key.clone();
        let label = key.clone();
        el.id(self.option_id(&key))
            .role(Role::ListBoxOption)
            .label(bind(move |r| self.collection.label(r, &label).unwrap_or_default()))
            .preserve_focus_on_press()
            .disabled_when(bind(move |r| !self.collection.is_enabled(r, &disabled)))
            .selected(bind(move |r| self.selection.contains(r, &selected)))
            .behavior_first(|behavior| {
                behavior.on_press(move |w| {
                    if composing.is_none_or(|composing| !w.get(composing)) {
                        self.commit(w, key.clone());
                    }
                })
            })
    }

    pub(crate) fn open(self, w: &mut WriteCx<'_>, fallback: Navigation) {
        if w.get(self.open).is_none() && w.get(self.available) {
            self.activate_initial(w, fallback);
            w.set(self.open, Some(fallback));
            // Opening may have mounted rows that can now supply their authored disabled state.
            w.request_command_after_commit(&self.id.part("check-initial-option"));
        }
    }

    fn activate_initial(self, w: &mut WriteCx<'_>, fallback: Navigation) {
        let selected = w.read(|r| self.selection.keys(r));
        let preferred = selected.into_iter().find(|key| w.read(|r| self.collection.is_enabled(r, key)) && self.can_activate(w, key));
        self.collection.activate_when(w, preferred, fallback, |w, key| self.can_activate(w, key));
    }

    pub(crate) fn close(self, w: &mut WriteCx<'_>) {
        w.set_if_changed(self.open, None);
        self.collection.set_active(w, None);
        self.collection.reset_typeahead(w);
    }

    pub(crate) fn navigate(self, w: &mut WriteCx<'_>, movement: Navigation) {
        self.collection.navigate_when(w, movement, |w, key| self.can_activate(w, key));
    }

    pub(crate) fn reset_active(self, w: &mut WriteCx<'_>) {
        self.collection.navigate(w, Navigation::First);
        w.request_command_after_commit(&self.id.part("reset-active-option"));
    }

    fn can_activate(self, w: &WriteCx<'_>, key: &K) -> bool {
        w.named_interactive(&self.option_id(key)) != Some(false)
    }

    fn active_is_disabled(self, w: &mut WriteCx<'_>) -> bool {
        w.read(|r| self.collection.active(r)).is_some_and(|key| !self.can_activate(w, &key))
    }

    pub(crate) fn commit_active(self, w: &mut WriteCx<'_>) {
        if let Some(key) = w.read(|r| self.collection.active(r)) {
            self.commit(w, key);
        }
    }

    fn commit(self, w: &mut WriteCx<'_>, key: K) {
        if w.get(self.open).is_none() {
            return;
        }
        if !self.can_activate(w, &key) {
            self.collection.set_active(w, None);
            return;
        }
        if self.collection.set_active(w, Some(key.clone())).is_some() {
            self.selection.select(w, key);
            if !self.selection.is_multiple() {
                self.close(w);
            }
        }
    }
}
