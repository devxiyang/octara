//! A keyed hierarchy flattened into the tree's currently visible rows.

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::hash::Hash;

use octara::prelude::{
    Binding, BuildCx, El, FocusGroup, HandlerKind, IntoReadList, Key, KeyEvent, Modifiers, ReadCx, ReadList, Role, RowSignal, Signal, View,
    VirtualOptions, WriteCx, each, virtual_each,
};

use crate::{
    Control,
    collection::{Collection, Selection},
    id::Id,
};

/// The hierarchy and behavior metadata for a source row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeItem<K> {
    pub parent: Option<K>,
    pub label: String,
    pub disabled: bool,
}

impl<K> TreeItem<K> {
    pub fn new(label: impl Into<String>) -> Self {
        Self { parent: None, label: label.into(), disabled: false }
    }

    pub fn under(mut self, parent: K) -> Self {
        self.parent = Some(parent);
        self
    }
}

/// A row in the visible, depth-first projection. Roots have depth zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow<K> {
    pub item: TreeItem<K>,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

/// Expansion, selection, and roving focus for a tree of stable business keys.
pub struct TreeViewState<K> {
    id: Id,
    collection: Collection<K, TreeRow<K>>,
    expanded: Binding<Vec<K>>,
    selection: Selection<K>,
    pending_focus: Signal<Option<K>>,
}

impl<K> Clone for TreeViewState<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for TreeViewState<K> {}

impl<K: Clone + Eq + Hash + 'static> TreeViewState<K> {
    /// Declares a tree over a reactive flat list. Siblings follow their source order.
    /// Rows become visible when their parent chain reaches a root and each ancestor is expanded.
    pub fn new(
        b: &mut BuildCx<'_>,
        items: impl IntoReadList<K, TreeItem<K>>,
        expanded: impl Into<Control<Vec<K>>>,
        selection: Selection<K>,
    ) -> Self {
        let items = items.into_read_list();
        let expanded = expanded.into().bind(b);
        let visible = b.derive_list(move |r| flatten(r, items, expanded));
        let collection = Collection::new(b, visible, |row| &row.item.label, |row| row.item.disabled);
        let pending_focus = b.signal(None::<K>);
        b.watch(
            move |r| r.get(pending_focus).is_some_and(|key| !collection.is_enabled(r, &key)),
            move |w, stale, _| {
                if *stale {
                    w.set(pending_focus, None);
                }
            },
        );
        Self { id: Id::new(), collection, expanded, selection, pending_focus }
    }

    pub fn visible_rows(self) -> ReadList<K, TreeRow<K>> {
        self.collection.rows()
    }

    pub fn active(self, r: &mut ReadCx<'_>) -> Option<K> {
        self.collection.active(r)
    }

    pub fn selection(self) -> Selection<K> {
        self.selection
    }

    pub fn expanded_keys(self) -> Binding<Vec<K>> {
        self.expanded
    }

    pub fn set_expanded(self, w: &mut WriteCx<'_>, key: &K, expanded: bool) {
        let mut keys = w.read(|r| self.expanded.get(r));
        if keys.contains(key) == expanded {
            return;
        }
        if expanded {
            keys.push(key.clone());
        } else {
            keys.retain(|candidate| candidate != key);
        }
        self.expanded.set(w, keys);
    }

    pub fn root(self, el: El) -> El {
        el.id(self.id.part("tree")).role(Role::Tree).multi_selectable(self.selection.is_multiple()).focus_group(FocusGroup::vertical())
    }
}

impl<K: Clone + Eq + Hash + Display + 'static> TreeViewState<K> {
    /// Mounts the visible rows on the tree element using the caller's row builder.
    pub fn rows(self, mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, TreeRow<K>>) -> El + 'static) -> View {
        each(self.visible_rows(), move |b, key, row| {
            let el = render(b, key, row);
            self.item(b, el, key)
        })
    }

    /// Place directly inside a vertical scroll container configured by `root`.
    /// Keys must have distinct display forms for Octara's reveal command.
    pub fn virtual_rows(
        self,
        options: VirtualOptions,
        mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, TreeRow<K>>) -> El + 'static,
    ) -> View {
        virtual_each(self.visible_rows(), options, move |b, key, row| {
            let el = render(b, key, row);
            self.item(b, el, key)
        })
    }

    /// Adds tree item semantics and interaction to an element in this tree.
    pub fn item(self, b: &mut BuildCx<'_>, el: El, key: &K) -> El {
        let id = Id::new().part("tree-item");
        let focus_id = id.clone();
        let requested = key.clone();
        b.watch(
            move |r| r.with(self.pending_focus, |pending| pending.as_ref() == Some(&requested)),
            move |w, pending, _| {
                // A resident row can request its own focus after the mount commits.
                if *pending {
                    w.set(self.pending_focus, None);
                    w.focus(&focus_id);
                }
            },
        );

        let row = self.visible_rows().row(key.clone());
        let label = row.clone();
        let expansion = row.clone();
        let level = row.clone();
        let active_key = key.clone();
        let selected_key = key.clone();
        let focus_key = key.clone();
        let press_key = key.clone();
        let keyboard_key = key.clone();
        el.id(id)
            .role(Role::TreeItem)
            .label(move |r: &mut ReadCx<'_>| label.with(r, |row| row.map_or_else(String::new, |row| row.item.label.clone())))
            .expanded_state(move |r: &mut ReadCx<'_>| expansion.with(r, |row| row.filter(|row| row.has_children).map(|row| row.expanded)))
            .level(move |r: &mut ReadCx<'_>| level.with(r, |row| row.map_or(1, |row| u8::try_from(row.depth + 1).unwrap_or(u8::MAX))))
            .disabled_when(move |r: &mut ReadCx<'_>| row.with(r, |row| row.is_none_or(|row| row.item.disabled)))
            .selected(move |r: &mut ReadCx<'_>| self.selection.contains(r, &selected_key))
            .focusable()
            .focus_preferred(move |r: &mut ReadCx<'_>| self.active(r).as_ref() == Some(&active_key))
            .behavior_first(|behavior| {
                behavior
                    .on(HandlerKind::Focus, move |w, _| {
                        self.collection.set_active(w, Some(focus_key.clone()));
                    })
                    .on_press(move |w| self.selection.select(w, press_key.clone()))
                    .on_key(move |w, event| self.on_key(w, &keyboard_key, event))
            })
    }

    /// Requests reveal for an enabled, logically visible row, then moves focus once it mounts.
    pub fn focus(self, w: &mut WriteCx<'_>, key: K) {
        if w.read(|r| self.collection.is_enabled(r, &key)) {
            self.collection.set_active(w, Some(key.clone()));
            self.focus_active(w, key);
        }
    }

    fn focus_active(self, w: &mut WriteCx<'_>, key: K) {
        w.reveal_row(&self.id.part("tree"), &key);
        w.set_if_changed(self.pending_focus, Some(key));
    }

    fn on_key(self, w: &mut WriteCx<'_>, key: &K, event: &KeyEvent) -> bool {
        if let Key::Char(character) = event.key
            && !character.is_control()
            && (event.modifiers - Modifiers::SHIFT).is_empty()
        {
            if let Some(key) = self.collection.typeahead(w, character) {
                self.focus_active(w, key);
            }
            return true;
        }
        if !event.modifiers.is_empty() {
            return false;
        }
        let next = match event.key {
            Key::Up | Key::Down | Key::Home | Key::End => w.read(|r| {
                let rows = self.collection.enabled();
                let rank = match event.key {
                    Key::Home => 0,
                    Key::End => r.len(rows).checked_sub(1)?,
                    Key::Up => r.rank(rows, key)?.checked_sub(1)?,
                    Key::Down => r.rank(rows, key)? + 1,
                    _ => unreachable!(),
                };
                r.row(rows, rank)
            }),
            Key::Left | Key::Right => {
                let Some(row) = w.get_row(self.visible_rows(), key) else { return false };
                match event.key {
                    Key::Left if row.expanded => {
                        self.set_expanded(w, key, false);
                        None
                    }
                    Key::Left => self.enabled_ancestor(w, row.item.parent),
                    Key::Right if row.expanded => {
                        let rows = self.visible_rows();
                        w.keys(self.collection.enabled()).into_iter().find(|candidate| {
                            w.with_row(rows, candidate, |row| row.is_some_and(|row| row.item.parent.as_ref() == Some(key)))
                        })
                    }
                    Key::Right => {
                        if row.has_children {
                            self.set_expanded(w, key, true);
                        }
                        None
                    }
                    _ => unreachable!(),
                }
            }
            Key::Enter | Key::Space => return event.repeat,
            _ => return false,
        };
        self.collection.reset_typeahead(w);
        if let Some(next) = next {
            self.focus(w, next);
        }
        true
    }

    fn enabled_ancestor(self, w: &mut WriteCx<'_>, mut parent: Option<K>) -> Option<K> {
        while let Some(key) = parent {
            let row = w.get_row(self.visible_rows(), &key)?;
            if !row.item.disabled {
                return Some(key);
            }
            parent = row.item.parent;
        }
        None
    }
}

fn flatten<K: Clone + Eq + Hash + 'static>(
    r: &mut ReadCx<'_>,
    items: ReadList<K, TreeItem<K>>,
    expanded: Binding<Vec<K>>,
) -> Vec<(K, TreeRow<K>)> {
    let expanded: HashSet<_> = expanded.get(r).into_iter().collect();
    let mut branches: HashMap<_, Vec<_>> = HashMap::new();
    for key in r.keys(items) {
        if let Some(item) = r.get_row(items, &key) {
            branches.entry(item.parent.clone()).or_default().push((key, item));
        }
    }
    let mut visible = Vec::new();
    let mut pending = Vec::new();
    if let Some(roots) = branches.remove(&None) {
        pending.extend(roots.into_iter().rev().map(|(key, item)| (key, item, 0)));
    }
    while let Some((key, item, depth)) = pending.pop() {
        let children = branches.remove(&Some(key.clone()));
        let has_children = children.is_some();
        let expanded = has_children && expanded.contains(&key);
        if let Some(children) = children.filter(|_| expanded) {
            pending.extend(children.into_iter().rev().map(|(key, item)| (key, item, depth + 1)));
        }
        visible.push((key, TreeRow { item, depth, has_children, expanded }));
    }
    visible
}
