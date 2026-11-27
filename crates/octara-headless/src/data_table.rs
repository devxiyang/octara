//! Keyed table rows and columns with unstyled sorting, selection, resize, and reorder parts.

use std::{cmp::Ordering, collections::HashSet, fmt::Display, hash::Hash};

use octara::prelude::{
    Binding, BuildCx, DragPayload, DropAxis, DropPosition, El, HandlerEvent, HandlerKind, InteractionPayload, IntoReadList, Key, Modifiers,
    ReadCx, ReadList, ReadSignal, Role, RowSignal, Signal, StateList, View, VirtualOptions, WriteCx, each, virtual_each,
};

use crate::{Control, collection::Selection, id::Id};

const RESIZE_STEP: f32 = 10.0;

/// The bounds and current width of a resizable column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnWidth {
    width: f32,
    min: f32,
    max: f32,
}

impl ColumnWidth {
    /// Requires a finite initial width and finite, ordered, nonnegative bounds.
    pub fn new(width: f32, min: f32, max: f32) -> Self {
        assert!(width.is_finite() && min.is_finite() && max.is_finite() && 0.0 <= min && min <= max);
        Self { width: width.clamp(min, max), min, max }
    }

    pub fn get(self) -> f32 {
        self.width
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub label: String,
    pub sortable: bool,
    pub width: Option<ColumnWidth>,
}

impl Column {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), sortable: false, width: None }
    }

    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }

    pub fn resizable(mut self, width: ColumnWidth) -> Self {
        self.width = Some(width);
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    #[default]
    Single,
    Multiple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<C> {
    pub column: C,
    pub direction: SortDirection,
}

/// A table controller with row keys `K` and typed column keys `C`.
pub struct DataTableState<K, C> {
    id: Id,
    columns: StateList<C, Column>,
    column_tokens: Signal<Vec<C>>,
    sort_mode: SortMode,
    sort: Binding<Vec<Sort<C>>>,
    effective_sort: ReadSignal<Vec<Sort<C>>>,
    selection: Option<Selection<K>>,
}

impl<K, C> Clone for DataTableState<K, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, C> Copy for DataTableState<K, C> {}

impl<K: Clone + Eq + 'static, C: Clone + Eq + Hash + 'static> DataTableState<K, C> {
    pub fn new(
        b: &mut BuildCx<'_>,
        columns: impl IntoIterator<Item = (C, Column)>,
        sort_mode: SortMode,
        sort: impl Into<Control<Vec<Sort<C>>>>,
        selection: Option<Selection<K>>,
    ) -> Self {
        let columns = b.list_from(columns);
        let keys = b.sample_list_keys(columns);
        let sort = sort.into().bind(b);
        let effective_sort = b.memo(move |r| {
            let mut seen = HashSet::new();
            let limit = if sort_mode == SortMode::Single { 1 } else { usize::MAX };
            r.get(sort)
                .into_iter()
                .filter(|sort| {
                    r.with_row(columns, &sort.column, |column| column.is_some_and(|column| column.sortable))
                        && seen.insert(sort.column.clone())
                })
                .take(limit)
                .collect()
        });
        Self { id: Id::new(), columns, column_tokens: b.signal(keys), sort_mode, sort, effective_sort, selection }
    }

    /// The reactive column order and metadata, keyed by column identity.
    pub fn columns(self) -> ReadList<C, Column> {
        self.columns.read_only()
    }

    /// The effective sort priorities, in order from highest to lowest.
    pub fn sort(self, r: &mut ReadCx<'_>) -> Vec<Sort<C>> {
        r.get(self.effective_sort)
    }

    pub fn sort_direction(self, r: &mut ReadCx<'_>, column: &C) -> Option<SortDirection> {
        r.with(self.effective_sort, |sort| sort.iter().find(|entry| &entry.column == column).map(|entry| entry.direction))
    }

    /// Cycles ascending, descending, and unsorted. Additive sorting keeps each
    /// column's priority and appends a newly sorted column at the end.
    pub fn cycle_sort(self, w: &mut WriteCx<'_>, column: &C, additive: bool) {
        if !w.with_row(self.columns, column, |column| column.is_some_and(|column| column.sortable)) {
            return;
        }
        let mut sort = w.get(self.effective_sort);
        let existing = sort.iter().position(|entry| &entry.column == column);
        let direction = match existing.map(|index| sort[index].direction) {
            None => Some(SortDirection::Ascending),
            Some(SortDirection::Ascending) => Some(SortDirection::Descending),
            Some(SortDirection::Descending) => None,
        };
        if !additive || self.sort_mode == SortMode::Single {
            sort.clear();
            sort.extend(direction.map(|direction| Sort { column: column.clone(), direction }));
        } else if let Some(index) = existing {
            if let Some(direction) = direction {
                sort[index].direction = direction;
            } else {
                sort.remove(index);
            }
        } else if let Some(direction) = direction {
            sort.push(Sort { column: column.clone(), direction });
        }
        w.set(self.sort, sort);
    }

    /// Reorders the source column relative to a live column, preserving both keys.
    /// Dropping in the center moves the source to the target's prior position.
    pub fn move_column(self, w: &mut WriteCx<'_>, source: &C, target: &C, position: DropPosition) {
        if source == target {
            return;
        }
        let order = w.keys(self.columns);
        let Some(from) = order.iter().position(|key| key == source) else { return };
        let Some(anchor) = order.iter().position(|key| key == target) else { return };
        let after = position == DropPosition::After || (position == DropPosition::Inside && from < anchor);
        w.move_row(self.columns, source, anchor - usize::from(from < anchor) + usize::from(after));
    }

    pub fn column_width(self, r: &mut ReadCx<'_>, column: &C) -> Option<f32> {
        r.with_row(self.columns, column, |column| column?.width.map(ColumnWidth::get))
    }

    pub fn resize_column(self, w: &mut WriteCx<'_>, column: &C, width: f32) {
        if !width.is_finite() {
            return;
        }
        let Some(current) = w.with_row(self.columns, column, |column| column?.width) else { return };
        let width = width.clamp(current.min, current.max);
        if width != current.width {
            w.update_row(self.columns, column, |column| column.width = Some(ColumnWidth { width, ..current }));
        }
    }

    pub fn selection(self) -> Option<Selection<K>> {
        self.selection
    }

    pub fn root(self, el: El) -> El {
        el.role(Role::Table)
    }

    /// Adds the reveal target to the scroll container that directly hosts virtual rows.
    pub fn body(self, el: El) -> El {
        el.id(self.id.part("body"))
    }

    pub fn row(self, el: El, key: &K) -> El {
        let el = el.role(Role::Row);
        let Some(selection) = self.selection else { return el };
        let selected = key.clone();
        let pressed = key.clone();
        el.focusable().selected(move |r: &mut ReadCx<'_>| selection.contains(r, &selected)).behavior_first(|behavior| {
            behavior
                .on_press(move |w| selection.select(w, pressed.clone()))
                .on_key(|_, key| key.repeat && key.modifiers.is_empty() && matches!(key.key, Key::Space | Key::Enter))
        })
    }

    pub fn cell(self, el: El) -> El {
        el.role(Role::Cell)
    }

    /// Enables scoped pointer and keyboard reordering on the actual sibling column headers.
    pub fn column_header(self, b: &mut BuildCx<'_>, el: El, column: &C) -> El {
        let el = el.role(Role::ColumnHeader);
        let Some(token) = b.read(|r| r.with(self.column_tokens, |columns| columns.iter().position(|key| key == column))) else { return el };
        let label = b.read(|r| r.with_row(self.columns, column, |column| column.expect("defined column").label.clone()));
        let target = column.clone();
        let kind = self.id.part("column");
        el.label(label.clone())
            .draggable(DragPayload::new(kind.clone(), token).preview_label(label))
            .drop_target([kind], DropAxis::Horizontal, move |w, drop| {
                if let Some(token) = drop.data.as_u64().and_then(|value| usize::try_from(value).ok())
                    && let Some(source) = w.with(self.column_tokens, |columns| columns.get(token).cloned())
                {
                    self.move_column(w, &source, &target, drop.position);
                }
            })
            .reorder_keys()
    }

    /// Adds a sort control inside a header. Shift activation adds to a multi-column sort.
    pub fn sort_button(self, el: El, column: &C) -> El {
        let press_column = column.clone();
        let key_column = column.clone();
        let disabled = column.clone();
        el.role(Role::Button)
            .focusable()
            .disabled_when(move |r: &mut ReadCx<'_>| {
                r.with_row(self.columns, &disabled, |column| column.is_none_or(|column| !column.sortable))
            })
            .behavior_first(|behavior| {
                behavior
                    .on(HandlerKind::Press, move |w, event| {
                        let additive = matches!(event, HandlerEvent::Interaction {
                            payload: InteractionPayload::Press { modifiers, .. }, ..
                        } if modifiers.contains(Modifiers::SHIFT));
                        self.cycle_sort(w, &press_column, additive);
                    })
                    .on_key(move |w, event| {
                        if !matches!(event.key, Key::Space | Key::Enter) || !(event.modifiers - Modifiers::SHIFT).is_empty() {
                            return false;
                        }
                        if event.modifiers.is_empty() {
                            return event.repeat;
                        }
                        if !event.repeat {
                            self.cycle_sort(w, &key_column, true);
                        }
                        true
                    })
            })
    }

    /// Adds a resize handle inside a header. The caller uses `column_width` to
    /// apply the chosen width to its own layout.
    pub fn resize_handle(self, b: &mut BuildCx<'_>, el: El, column: &C) -> El {
        let Some(initial) = b.read(|r| r.with_row(self.columns, column, |column| column?.width)) else { return el };
        let key = column.clone();
        let write_key = key.clone();
        let width = b.binding(
            move |read| self.column_width(read, &key).unwrap_or(initial.width),
            move |write, width| self.resize_column(write, &write_key, width),
        );
        crate::splitter::SplitterState::with_steps(b, width, RESIZE_STEP, None).bounded(el, initial.min, initial.max)
    }
}

impl<K: Clone + Eq + Hash + 'static, C: Clone + Eq + Hash + 'static> DataTableState<K, C> {
    /// Produces a stable sorted projection. The comparator reads the caller's row
    /// values and chooses the comparison for the supplied typed column key.
    pub fn sorted_rows<T: Clone + PartialEq + 'static>(
        self,
        b: &mut BuildCx<'_>,
        rows: impl IntoReadList<K, T>,
        compare: impl Fn(&C, &T, &T) -> Ordering + 'static,
    ) -> ReadList<K, T> {
        let rows = rows.into_read_list();
        b.derive_list(move |r| {
            let sort = self.sort(r);
            let mut entries: Vec<_> = r.keys(rows).into_iter().filter_map(|key| r.get_row(rows, &key).map(|row| (key, row))).collect();
            if !sort.is_empty() {
                entries.sort_by(|(_, a), (_, b)| {
                    sort.iter()
                        .map(|sort| match sort.direction {
                            SortDirection::Ascending => compare(&sort.column, a, b),
                            SortDirection::Descending => compare(&sort.column, b, a),
                        })
                        .find(|&order| order != Ordering::Equal)
                        .unwrap_or(Ordering::Equal)
                });
            }
            entries
        })
    }
}

impl<K: Clone + Eq + Hash + Display + 'static, C: Clone + Eq + Hash + 'static> DataTableState<K, C> {
    pub fn rows<T: 'static>(
        self,
        rows: impl IntoReadList<K, T>,
        mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<T>) -> El + 'static,
    ) -> View {
        each(rows, move |b, key, row| self.row(render(b, key, row), key))
    }

    /// Mounts only the rows near the viewport, plus any rows pinned by focus or capture.
    /// Place directly inside a vertical scroll container configured by `body`.
    pub fn virtual_rows<T: 'static>(
        self,
        rows: impl IntoReadList<K, T>,
        options: VirtualOptions,
        mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<T>) -> El + 'static,
    ) -> View {
        virtual_each(rows, options, move |b, key, row| self.row(render(b, key, row), key))
    }

    pub fn reveal_row(self, w: &mut WriteCx<'_>, key: &K) {
        w.reveal_row(&self.id.part("body"), key);
    }
}
