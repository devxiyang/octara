//! Themed rows, column controls and scroll surfaces for the headless data controllers.

use crate::{ButtonVariant, KitLocale, Size, TOUCH_TARGET, parts::button_part, row_value};
use octara::prelude::*;
use octara_headless::{DataTableState, TreeViewState, data_table::SortDirection, tree_view::TreeRow};
use std::{cmp::Ordering, fmt::Display as KeyDisplay, hash::Hash};

/// Renders a tree with indentation and a disclosure affordance on branch rows.
#[ui]
#[track_caller]
pub fn tree_view<K: Clone + Eq + Hash + KeyDisplay + 'static>(state: TreeViewState<K>, options: Option<VirtualOptions>) -> El {
    tree_view_with(state, options, move |_, key, item| {
        let key = key.clone();
        let toggle = keyed(
            move |read| match row_value(item, read, |row| (row.has_children, row.expanded)) {
                (false, _) => "leaf",
                (true, true) => "expanded",
                (true, false) => "collapsed",
            },
            move |_, branch| {
                if *branch == "leaf" {
                    return spacer().grow(0.0).w(sp(6.5)).a11y_hidden();
                }
                let glyph = if *branch == "expanded" { octara_icons::CHEVRON_DOWN } else { octara_icons::CHEVRON_RIGHT };
                let key = key.clone();
                el().size(26.0)
                    .when(When::Touch, |style| style.min_w(TOUCH_TARGET).min_h(TOUCH_TARGET))
                    .align(AlignItems::Center)
                    .justify(JustifyContent::Center)
                    .rounded(RadiusToken::Sm)
                    .when(State::HOVER, |style| style.bg(ColorToken::SurfaceHover))
                    .a11y_hidden()
                    .preserve_focus_on_press()
                    .child(icon(glyph.into()).size(sp(3.0)))
                    .on_press(move |write| {
                        let expanded = write.try_with(item, |row| row.expanded).unwrap_or_default();
                        state.set_expanded(write, &key, !expanded);
                    })
            },
        );
        row()
            .align(AlignItems::Center)
            .gap(sp(1.0))
            .style_with(move |read, style| style.pl(sp(2.0 + 4.0 * row_value(item, read, |row| row.depth) as f32)))
            .child(toggle)
            .child(text(bind(move |read| row_value(item, read, |row| row.item.label.clone()))).min_w(0.0))
    })
}

/// Renders caller-authored rows with Kit row styling and optional virtualization.
#[ui]
#[track_caller]
pub fn tree_view_with<K: Clone + Eq + Hash + KeyDisplay + 'static>(
    state: TreeViewState<K>,
    options: Option<VirtualOptions>,
    mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<TreeRow<K>>) -> El + 'static,
) -> El {
    let root = col().gap(sp(0.5)).overflow_y(Overflow::Scroll).min_w(0.0);
    let render = move |build: &mut BuildCx<'_>, key: &K, row| style_row(render(build, key, row)).rounded(RadiusToken::Sm);
    match options {
        Some(options) => state.root(root.h(320.0)).child(state.virtual_rows(options, render)),
        None => state.root(root).child(state.rows(render)),
    }
}

/// Lays out sortable headers, column resize handles and caller-authored cells.
#[ui]
#[track_caller]
pub fn data_table<K, C, T>(
    build: &mut BuildCx<'_>,
    state: DataTableState<K, C>,
    rows: ReadList<K, T>,
    options: Option<VirtualOptions>,
    compare: impl Fn(&C, &T, &T) -> Ordering + 'static,
    render_cell: impl Fn(&mut BuildCx<'_>, &C, &K, RowSignal<T>) -> El + Clone + 'static,
) -> El
where
    K: Clone + Eq + Hash + KeyDisplay + 'static,
    C: Clone + Eq + Hash + KeyDisplay + 'static,
    T: Clone + PartialEq + 'static,
{
    let sorted = state.sorted_rows(build, rows, compare);
    let columns = state.columns();
    let locale = KitLocale::get(build);
    let headers = row().role(Role::Row).bg(ColorToken::Surface).border(1.0).border_color(Color::TRANSPARENT).child(each(
        columns,
        move |build, column, source| {
            let sort = column.clone();
            let divider = column.clone();
            let has_divider = build.read(|read| read.with(source, |value| value.width.is_some()));
            let sortable = build.read(|read| read.with(source, |value| value.sortable));
            let title = text(bind(move |read| row_value(source, read, |col| col.label.clone())))
                .min_w(0.0)
                .text_size(FontSizeToken::Sm)
                .text_weight(500);
            let content = if sortable {
                state.sort_button(
                    button_part(
                        row().align(AlignItems::Center).gap(sp(1.0)).child(title).child(
                            text(bind(move |read| match state.sort_direction(read, &sort) {
                                Some(SortDirection::Ascending) => "↑",
                                Some(SortDirection::Descending) => "↓",
                                None => "",
                            }))
                            .a11y_hidden(),
                        ),
                        Size::Sm,
                        ButtonVariant::Ghost,
                    )
                    .justify(JustifyContent::Start)
                    .label(bind(move |read| locale.sort_column.replace("{item}", &row_value(source, read, |column| column.label.clone())))),
                    column,
                )
            } else {
                title.px(sp(2.0)).min_h(26.0)
            };
            let mut header = row().align(AlignItems::Center).child(content.grow(1.0).min_w(0.0));
            if has_divider {
                header = header.child(
                    state.resize_handle(
                        build,
                        el().label(bind(move |read| {
                            locale.resize_column.replace("{item}", &row_value(source, read, |column| column.label.clone()))
                        }))
                        .w(7.0)
                        .when(When::Touch, |style| style.min_w(TOUCH_TARGET).min_h(TOUCH_TARGET))
                        .h(rel(1.0))
                        .min_h(26.0)
                        .justify(JustifyContent::Center)
                        .cursor(CursorStyle::ResizeLeftRight)
                        .child(el().w(1.0).h(rel(1.0)).bg(ColorToken::Border).a11y_hidden()),
                        &divider,
                    ),
                );
            }
            state.column_header(build, column_layout(header, state, column.clone()), column)
        },
    ));
    let render = move |_: &mut BuildCx<'_>, key: &K, value: RowSignal<T>| {
        let key = key.clone();
        let render = render_cell.clone();
        style_row(row().child(each(columns, move |build, column, _| {
            let cell = render(build, column, &key, value).px(sp(2.0)).min_w(0.0);
            state.cell(column_layout(cell, state, column.clone()))
        })))
    };
    let rows = match options {
        Some(options) => state.virtual_rows(sorted, options, render),
        None => state.rows(sorted, render),
    };
    let body = state.body(col().grow(1.0).min_h(0.0).overflow_y(Overflow::Scroll)).child(rows);
    let table = col()
        .border(1.0)
        .border_color(ColorToken::Border)
        .rounded(RadiusToken::Md)
        .overflow_x(Overflow::Hidden)
        .overflow_y(Overflow::Hidden)
        .child(headers)
        .child(crate::separator())
        .child(body);
    state.root(if options.is_some() { table.h(360.0) } else { table })
}

fn column_layout<K, C>(element: El, state: DataTableState<K, C>, column: C) -> El
where
    K: Clone + Eq + 'static,
    C: Clone + Eq + Hash + 'static,
{
    element.min_w(0.0).style_with(move |read, style| {
        if let Some(width) = state.column_width(read, &column) { style.w(width).shrink(0.0).grow(0.0) } else { style.grow(1.0).basis(0.0) }
    })
}

fn style_row(element: El) -> El {
    element
        .min_h(30.0)
        .when(When::Touch, |style| style.min_h(TOUCH_TARGET))
        .text_size(FontSizeToken::Sm)
        .border(1.0)
        .border_color(Color::TRANSPARENT)
        .when(State::HOVER, |style| style.bg(ColorToken::SurfaceHover))
        .when(State::SELECTED, |style| style.bg(ColorToken::SurfacePressed))
        .when(State::FOCUS_VISIBLE, |style| style.border_color(ColorToken::Focus))
        .when(State::DISABLED, |style| style.opacity(0.5))
}
