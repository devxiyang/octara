//! Keyed navigation and selection shared by composite controls.

use std::{fmt::Display, hash::Hash, time::Duration};

use octara::prelude::*;

use crate::Control;

/// A simple option model for pickers and radio groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub disabled: bool,
}

impl Choice {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), description: String::new(), disabled: false }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A movement within the enabled rows of a collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    Previous,
    Next,
    First,
    Last,
}

/// Tracks a logical active key independently of the rows currently mounted.
pub struct Collection<K, T> {
    rows: ReadList<K, T>,
    enabled: ReadList<K, T>,
    labels: ReadList<K, (String, String)>,
    active: Signal<Option<K>>,
    activation: Signal<u64>,
    search: Signal<(String, Duration)>,
}

impl<K, T> Copy for Collection<K, T> {}

impl<K, T> Clone for Collection<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + 'static> Collection<K, Choice> {
    pub fn choices(b: &mut BuildCx<'_>, rows: impl IntoReadList<K, Choice>) -> Self {
        Self::new(b, rows, |row| &row.label, |row| row.disabled)
    }
}

impl<K: Clone + Eq + Hash + 'static, T: 'static> Collection<K, T> {
    pub fn new(
        b: &mut BuildCx<'_>,
        rows: impl IntoReadList<K, T>,
        label: impl Fn(&T) -> &str + 'static,
        disabled: impl Fn(&T) -> bool + 'static,
    ) -> Self {
        let rows = rows.into_read_list();
        let enabled = rows.filter(b, move |row| !disabled(row));
        let labels = rows.map(b, move |row| {
            let label = label(row);
            (label.to_owned(), label.trim_start().to_lowercase())
        });
        let active = b.signal(None::<K>);
        let activation = b.signal(0_u64);
        let search = b.signal((String::new(), Duration::ZERO));
        b.watch(
            move |r| r.get(active).filter(|key| r.rank(enabled, key).is_none()),
            move |w, stale, _| {
                if let Some(key) = stale
                    && w.read(|r| r.get(active).as_ref() == Some(key) && r.rank(enabled, key).is_none())
                {
                    w.set_if_changed(active, None);
                }
            },
        );
        Self { rows, enabled, labels, active, activation, search }
    }

    pub fn rows(self) -> ReadList<K, T> {
        self.rows
    }

    pub fn enabled(self) -> ReadList<K, T> {
        self.enabled
    }

    /// Whether the collection has no available choices.
    pub fn is_empty(self, r: &mut ReadCx<'_>) -> bool {
        r.len(self.enabled) == 0
    }

    pub fn is_enabled(self, r: &mut ReadCx<'_>, key: &K) -> bool {
        r.rank(self.enabled, key).is_some()
    }

    pub fn label(self, r: &mut ReadCx<'_>, key: &K) -> Option<String> {
        r.with_row(self.labels, key, |row| row.map(|(label, _)| label.clone()))
    }

    pub fn active(self, r: &mut ReadCx<'_>) -> Option<K> {
        r.get(self.active).filter(|key| self.is_enabled(r, key))
    }

    pub fn set_active(self, w: &mut WriteCx<'_>, key: Option<K>) -> Option<K> {
        let key = key.filter(|key| w.read(|r| self.is_enabled(r, key)));
        w.set_if_changed(self.active, key.clone());
        if key.is_some() {
            // An explicit jump can reveal the same logical key after manual scrolling.
            w.update(self.activation, |revision| *revision = revision.wrapping_add(1));
        }
        key
    }

    /// Moves the active key by one, wrapping at either end, or jumps to an endpoint.
    pub fn navigate(self, w: &mut WriteCx<'_>, movement: Navigation) -> Option<K> {
        self.navigate_when(w, movement, |_, _| true)
    }

    /// Navigates through the enabled keys that also pass a caller's interaction check.
    pub fn navigate_when(self, w: &mut WriteCx<'_>, movement: Navigation, eligible: impl Fn(&WriteCx<'_>, &K) -> bool) -> Option<K> {
        self.reset_typeahead(w);
        let (len, current) = w.read(|r| {
            let current = r.get(self.active).and_then(|key| r.rank(self.enabled, &key));
            (r.len(self.enabled), current)
        });
        if len == 0 {
            return self.set_active(w, None);
        }
        let backward = matches!(movement, Navigation::Previous | Navigation::Last);
        let mut index = match (movement, current) {
            (Navigation::First, _) | (Navigation::Next, None) => 0,
            (Navigation::Last, _) | (Navigation::Previous, Some(0) | None) => len - 1,
            (Navigation::Next, Some(index)) => (index + 1) % len,
            (Navigation::Previous, Some(index)) => index - 1,
        };
        let next = (0..len).find_map(|_| {
            let key = w.read(|r| r.row(self.enabled, index));
            index = if backward { index.checked_sub(1).unwrap_or(len - 1) } else { (index + 1) % len };
            key.filter(|key| eligible(w, key))
        });
        self.set_active(w, next)
    }

    /// Activates an enabled preferred key, or navigates from an unset active key.
    pub fn activate(self, w: &mut WriteCx<'_>, preferred: Option<K>, fallback: Navigation) -> Option<K> {
        self.activate_when(w, preferred, fallback, |_, _| true)
    }

    /// Applies a caller's interaction check to both the preferred key and fallback.
    pub fn activate_when(
        self,
        w: &mut WriteCx<'_>,
        preferred: Option<K>,
        fallback: Navigation,
        eligible: impl Fn(&WriteCx<'_>, &K) -> bool,
    ) -> Option<K> {
        self.reset_typeahead(w);
        let preferred = preferred.filter(|key| eligible(w, key));
        self.set_active(w, preferred).or_else(|| self.navigate_when(w, fallback, eligible))
    }

    /// Extends a case-insensitive prefix within 500 ms. A repeated initial cycles through matches.
    pub fn typeahead(self, w: &mut WriteCx<'_>, character: char) -> Option<K> {
        self.typeahead_when(w, character, |_, _| true)
    }

    /// Searches enabled rows that also pass a caller's interaction check.
    pub fn typeahead_when(self, w: &mut WriteCx<'_>, character: char, eligible: impl Fn(&WriteCx<'_>, &K) -> bool) -> Option<K> {
        if character.is_control() {
            return None;
        }
        let now = w.now();
        let (mut prefix, last) = w.get(self.search);
        if now.saturating_sub(last) > Duration::from_millis(500) {
            prefix.clear();
        }
        let initial = prefix.is_empty();
        let character: String = character.to_lowercase().collect();
        prefix.push_str(&character);
        let next = self.match_prefix(w, &prefix, initial, &eligible);
        let next = if next.is_none() && prefix.len() > character.len() && prefix.replace(&character, "").is_empty() {
            prefix = character;
            self.match_prefix(w, &prefix, true, &eligible)
        } else {
            next
        };
        w.set(self.search, (prefix, now));
        next.and_then(|key| self.set_active(w, Some(key)))
    }

    pub fn reset_typeahead(self, w: &mut WriteCx<'_>) {
        w.set_if_changed(self.search, (String::new(), Duration::ZERO));
    }

    fn match_prefix(self, w: &mut WriteCx<'_>, prefix: &str, advance: bool, eligible: &impl Fn(&WriteCx<'_>, &K) -> bool) -> Option<K> {
        let (keys, start) = w.read(|r| {
            let start = self.active(r).and_then(|key| r.rank(self.enabled, &key)).map_or(0, |index| index + usize::from(advance));
            (r.keys(self.enabled), start)
        });
        let (before, after) = keys.split_at(start);
        after
            .iter()
            .chain(before)
            .find(|key| w.with_row(self.labels, key, |row| row.is_some_and(|(_, label)| label.starts_with(prefix))) && eligible(w, key))
            .cloned()
    }

    /// Reveals the active key in a virtual list after its container is ready to scroll.
    ///
    /// Attach `geometry` to the element that directly contains `virtual_each`. Virtual row keys
    /// must have distinct display forms for Octara's reveal command.
    pub fn observe_virtual(self, b: &mut BuildCx<'_>, container: impl Into<String>, geometry: Signal<Option<LayoutBox>>)
    where
        K: Display,
    {
        let container = container.into();
        b.watch(
            move |r| {
                let active = r.get(geometry).is_some().then(|| self.active(r)).flatten();
                (active, r.get(self.activation), r.list_topology_cursor(self.rows))
            },
            move |w, (active, _, _), _| {
                if let Some(active) = active {
                    w.reveal_row(&container, active);
                }
            },
        );
    }
}

/// A controlled or scope-owned selection containing business keys.
pub enum Selection<K> {
    Single(Binding<Option<K>>),
    Multiple(Binding<Vec<K>>),
}

impl<K> Copy for Selection<K> {}

impl<K> Clone for Selection<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + 'static> Selection<K> {
    pub fn single(b: &mut BuildCx<'_>, value: impl Into<Control<Option<K>>>) -> Self {
        Self::Single(value.into().bind(b))
    }

    pub fn multiple(b: &mut BuildCx<'_>, value: impl Into<Control<Vec<K>>>) -> Self {
        Self::Multiple(value.into().bind(b))
    }

    pub fn is_multiple(self) -> bool {
        matches!(self, Self::Multiple(_))
    }

    pub fn contains(self, r: &mut ReadCx<'_>, key: &K) -> bool {
        match self {
            Self::Single(value) => value.with(r, |value| value.as_ref() == Some(key)),
            Self::Multiple(value) => value.with(r, |value| value.contains(key)),
        }
    }

    pub fn keys(self, r: &mut ReadCx<'_>) -> Vec<K> {
        match self {
            Self::Single(value) => value.get(r).into_iter().collect(),
            Self::Multiple(value) => value.get(r),
        }
    }

    /// Proposes a single selection or toggles a key in a multiple selection.
    pub fn select(self, w: &mut WriteCx<'_>, key: K) {
        match self {
            Self::Single(value) => {
                if !w.read(|r| self.contains(r, &key)) {
                    value.set(w, Some(key));
                }
            }
            Self::Multiple(value) => {
                let mut selected = w.read(|r| value.get(r));
                if selected.contains(&key) {
                    selected.retain(|selected| selected != &key);
                } else {
                    selected.push(key);
                }
                value.set(w, selected);
            }
        }
    }
}
