//! A single-selection radio group with roving focus.

use std::hash::Hash;

use octara::prelude::*;

use crate::{
    Control,
    collection::{Choice, Collection, Selection},
};

/// Coordinates a group of radios whose values are business keys.
pub struct RadioGroupState<K, T = Choice> {
    collection: Collection<K, T>,
    value: Binding<Option<K>>,
}

impl<K, T> Copy for RadioGroupState<K, T> {}

impl<K, T> Clone for RadioGroupState<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + 'static> RadioGroupState<K> {
    pub fn new(b: &mut BuildCx<'_>, rows: impl IntoReadList<K, Choice>, value: impl Into<Control<Option<K>>>) -> Self {
        let collection = Collection::choices(b, rows);
        Self::with_collection(collection, value.into().bind(b))
    }
}

impl<K: Clone + Eq + Hash + 'static, T: 'static> RadioGroupState<K, T> {
    pub fn with_collection(collection: Collection<K, T>, value: Binding<Option<K>>) -> Self {
        Self { collection, value }
    }

    pub fn value(self) -> Binding<Option<K>> {
        self.value
    }

    /// Attaches group semantics and arrow-key selection to the caller's container.
    pub fn group<C>(self, el: El<C>) -> El<C> {
        el.role(Role::RadioGroup).focus_group(FocusGroup::both().wrap().activate_on_navigation())
    }

    pub fn radio<C>(self, el: El<C>, key: K) -> El<C> {
        let selection = Selection::Single(self.value);
        let disabled = key.clone();
        let preferred = key.clone();
        let checked = key.clone();
        let label = key.clone();
        el.role(Role::RadioButton)
            .label(bind(move |r| self.collection.label(r, &label).unwrap_or_default()))
            .focusable()
            .disabled_when(bind(move |r| !self.collection.is_enabled(r, &disabled)))
            .focus_preferred(bind(move |r| selection.contains(r, &preferred)))
            .checked(bind(move |r| selection.contains(r, &checked)))
            .behavior_first(|behavior| {
                behavior.on_press(move |w| {
                    if w.read(|r| self.collection.is_enabled(r, &key)) {
                        selection.select(w, key.clone());
                    }
                })
            })
    }
}
