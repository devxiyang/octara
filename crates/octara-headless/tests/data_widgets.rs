#![cfg(not(target_arch = "wasm32"))]

use std::fmt;

use octara::{Input, InteractionEvent, NodeId, PointerId, PointerKind, Tree, WindowRuntime, prelude::*};
use octara_headless::{
    Control,
    collection::Selection,
    data_table::{Column, ColumnWidth, DataTableState, Sort, SortDirection, SortMode},
    tree_view::{TreeItem, TreeViewState},
};
use octara_inspect::HeadlessDriver;

const VIRTUAL: VirtualOptions = VirtualOptions { estimated_row_height: 20.0, overscan: 0, maintain_scroll_anchor: false };

fn runtime(driver: &HeadlessDriver, window: WindowId) -> &WindowRuntime {
    driver.app().window(window).unwrap().runtime()
}

fn tree(driver: &HeadlessDriver, window: WindowId) -> &Tree {
    runtime(driver, window).tree()
}

fn node(driver: &HeadlessDriver, window: WindowId, test_id: &str) -> NodeId {
    let [id] = tree(driver, window).test_ids(test_id) else { panic!("expected one node with test ID {test_id}") };
    *id
}

fn key(driver: &mut HeadlessDriver, window: WindowId, key: Key, modifiers: Modifiers) {
    driver.dispatch(window, Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, modifiers)))).unwrap();
}

fn focus(driver: &mut HeadlessDriver, window: WindowId, test_id: &str) {
    driver.app_mut().update_window(window, |w| w.focus(test_id)).unwrap();
    assert_eq!(tree(driver, window).focus(), Some(node(driver, window, test_id)));
}

fn window_point(driver: &HeadlessDriver, window: WindowId, node: NodeId, offset: Point) -> Point {
    let runtime = runtime(driver, window);
    std::iter::successors(Some(node), |&node| runtime.tree().physical_parent(node))
        .filter_map(|node| runtime.geometry(node))
        .fold(offset, |point, bounds| Point::new(point.x + bounds.rect.x, point.y + bounds.rect.y))
}

fn pointer_drag(driver: &mut HeadlessDriver, window: WindowId, from: Point, to: Point, cancel: bool) {
    let id = PointerId(1);
    let mods = Modifiers::empty();
    let target = runtime(driver, window).hit_test(from).expect("start hits test element");
    driver
        .dispatch(
            window,
            Input::Interaction(InteractionEvent::PointerDown {
                id,
                kind: PointerKind::Mouse,
                target,
                pos: from,
                button: Button::Primary,
                mods,
            }),
        )
        .unwrap();
    driver.dispatch(window, Input::Interaction(InteractionEvent::PointerMove { id, pos: to, mods })).unwrap();
    let end = if cancel {
        InteractionEvent::PointerCancel { id }
    } else {
        let target = runtime(driver, window).hit_test(to);
        InteractionEvent::PointerUp { id, target, pos: to, mods }
    };
    driver.dispatch(window, Input::Interaction(end)).unwrap();
}

#[test]
fn tree_projects_reactive_children_and_keeps_focus_by_key() {
    let mut driver = HeadlessDriver::default();
    let (source, expanded, selected, state) = driver
        .app_mut()
        .build(|b| {
            let source = b.list_from([
                (11, TreeItem { disabled: true, ..TreeItem::new("Duplicate").under(10) }),
                (10, TreeItem::new("Parent")),
                (12, TreeItem::new("Duplicate").under(10)),
                (20, TreeItem::new("Sibling")),
                (99, TreeItem::new("Orphan").under(100)),
            ]);
            let expanded = b.signal(Vec::new());
            let selected = b.signal(None);
            let state = TreeViewState::new(b, source, expanded, Selection::Single(selected.into()));
            (source, expanded, selected, state)
        })
        .unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |_| {
            state.root(col().test_id("tree")).child(state.rows(|_, key, _| el().h(20).test_id(format!("item-{key}"))))
        })
        .unwrap();

    let root = node(&driver, window, "tree");
    let parent = node(&driver, window, "item-10");
    assert_eq!(tree(&driver, window).semantics(root).unwrap().role, Some(Role::Tree));
    assert_eq!(tree(&driver, window).semantics(root).unwrap().group_current, Some(parent));
    assert_eq!(tree(&driver, window).semantics(parent).unwrap().expanded, Some(false));
    assert!(tree(&driver, window).test_ids("item-12").is_empty());
    assert!(tree(&driver, window).test_ids("item-99").is_empty());

    key(&mut driver, window, Key::Tab, Modifiers::empty());
    key(&mut driver, window, Key::Right, Modifiers::empty());
    assert_eq!(tree(&driver, window).focus(), Some(parent));
    assert_eq!(driver.app_mut().update(|w| w.get(expanded)).unwrap(), [10]);
    assert_eq!(tree(&driver, window).semantics(parent).unwrap().expanded, Some(true));
    key(&mut driver, window, Key::Right, Modifiers::empty());
    let child = node(&driver, window, "item-12");
    assert_eq!(tree(&driver, window).focus(), Some(child));
    assert_eq!(tree(&driver, window).semantics(root).unwrap().group_current, Some(child));
    let semantics = tree(&driver, window).semantics(child).unwrap();
    assert_eq!(semantics.role, Some(Role::TreeItem));
    assert_eq!(semantics.expanded, None);
    assert_eq!(semantics.level, Some(2));
    assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), None);

    key(&mut driver, window, Key::Space, Modifiers::empty());
    assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), Some(12));
    assert_eq!(tree(&driver, window).semantics(child).unwrap().selected, Some(true));

    driver
        .app_mut()
        .update(|w| {
            w.move_row(source, &12, 0);
            w.update_row(source, &12, |row| row.label = "Renamed".into());
        })
        .unwrap();
    assert_eq!(node(&driver, window, "item-12"), child);
    assert_eq!(tree(&driver, window).focus(), Some(child));
    assert_eq!(tree(&driver, window).semantics(child).unwrap().label.as_deref(), Some("Renamed"));

    key(&mut driver, window, Key::Left, Modifiers::empty());
    assert_eq!(tree(&driver, window).focus(), Some(parent));
    key(&mut driver, window, Key::Left, Modifiers::empty());
    assert!(tree(&driver, window).test_ids("item-12").is_empty());
    assert_eq!(driver.app_mut().update(|w| w.keys(state.visible_rows())).unwrap(), [10, 20]);
    driver.app_mut().update(|w| w.remove_row(source, &10)).unwrap();
    let sibling = node(&driver, window, "item-20");
    assert_eq!(tree(&driver, window).semantics(root).unwrap().group_current, Some(sibling));
}

#[test]
fn virtual_tree_moves_real_focus_to_missing_rows_and_toggles_multi_selection() {
    let mut driver = HeadlessDriver::default();
    let (state, selected) = driver
        .app_mut()
        .build(|b| {
            let items = b.list_from((0..100).map(|key| (key, TreeItem::new(format!("Item {key:03}")))));
            let selected = b.signal(Vec::new());
            let state = TreeViewState::new(b, items, Control::Uncontrolled(Vec::new()), Selection::Multiple(selected.into()));
            (state, selected)
        })
        .unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |_| {
            state
                .root(col().h(60).overflow_y(Overflow::Scroll))
                .child(state.virtual_rows(VIRTUAL, |_, key, _| el().h(20).test_id(format!("item-{key}"))))
        })
        .unwrap();

    assert!(tree(&driver, window).test_ids("item-99").is_empty());
    key(&mut driver, window, Key::Tab, Modifiers::empty());
    assert_eq!(tree(&driver, window).focus(), Some(node(&driver, window, "item-0")));
    key(&mut driver, window, Key::Space, Modifiers::empty());
    key(&mut driver, window, Key::End, Modifiers::empty());
    assert_eq!(tree(&driver, window).focus(), Some(node(&driver, window, "item-99")));
    assert!(tree(&driver, window).test_ids("item-45").is_empty());
    key(&mut driver, window, Key::Enter, Modifiers::empty());
    assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), [0, 99]);
    key(&mut driver, window, Key::Home, Modifiers::empty());
    assert_eq!(tree(&driver, window).focus(), Some(node(&driver, window, "item-0")));
    key(&mut driver, window, Key::Space, Modifiers::empty());
    assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), [99]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum C {
    Category,
    Score,
}

impl fmt::Display for C {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("column")
    }
}

fn columns() -> [(C, Column); 2] {
    [
        (C::Category, Column::new("Duplicate").sortable().resizable(ColumnWidth::new(100.0, 60.0, 150.0))),
        (C::Score, Column::new("Duplicate").sortable()),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Record {
    category: u32,
    score: u32,
}

#[test]
fn table_sorts_keyed_virtual_rows_and_reorders_actual_headers_with_typed_keys() {
    let mut driver = HeadlessDriver::default();
    let (table, sorted, selected) = driver
        .app_mut()
        .build(|b| {
            let rows = b.list_from((0..25).map(|key| (key, Record { category: key % 2, score: 25 - key / 2 })));
            let selected = b.signal(None);
            let table = DataTableState::new(
                b,
                columns(),
                SortMode::Multiple,
                Control::Uncontrolled(Vec::new()),
                Some(Selection::Single(selected.into())),
            );
            let sorted = table.sorted_rows(b, rows, |column, a, b| match column {
                C::Category => a.category.cmp(&b.category),
                C::Score => a.score.cmp(&b.score),
            });
            (table, sorted, selected)
        })
        .unwrap();

    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |_| {
            table
                .root(col())
                .child(row().child(each(table.columns(), move |b, column, _| {
                    let resize = (*column == C::Category).then(|| table.resize_handle(b, el().w(30).h(20).test_id("resize"), column));
                    let sort = table.sort_button(el().w(30).h(20).test_id(format!("sort-{column:?}")), column);
                    let header = row().w(100).h(20).test_id(format!("header-{column:?}")).child(resize).child(sort);
                    table.column_header(b, header, column)
                })))
                .child(
                    table
                        .body(col().h(40).overflow_y(Overflow::Scroll))
                        .child(table.virtual_rows(sorted, VIRTUAL, |_, key, _| el().h(20).test_id(format!("record-{key}")))),
                )
        })
        .unwrap();

    let category = node(&driver, window, "header-Category");
    let score = node(&driver, window, "header-Score");
    assert_eq!(tree(&driver, window).semantics(category).unwrap().role, Some(Role::ColumnHeader));
    assert!(tree(&driver, window).test_ids("record-24").is_empty());
    driver.app_mut().update_window(window, |w| table.reveal_row(w, &24)).unwrap();
    focus(&mut driver, window, "record-24");
    key(&mut driver, window, Key::Space, Modifiers::empty());
    assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), Some(24));

    focus(&mut driver, window, "sort-Category");
    key(&mut driver, window, Key::Enter, Modifiers::empty());
    let order = driver.app_mut().update(|w| w.keys(sorted)).unwrap();
    assert_eq!(&order[..5], &[0, 2, 4, 6, 8]);
    focus(&mut driver, window, "sort-Score");
    key(&mut driver, window, Key::Enter, Modifiers::SHIFT);
    let order = driver.app_mut().update(|w| w.keys(sorted)).unwrap();
    assert_eq!(&order[..5], &[24, 22, 20, 18, 16]);
    assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), Some(24));

    focus(&mut driver, window, "header-Category");
    key(&mut driver, window, Key::Space, Modifiers::empty());
    key(&mut driver, window, Key::Right, Modifiers::empty());
    assert!(tree(&driver, window).node(score).unwrap().state.contains(State::DROP_HOVER));
    key(&mut driver, window, Key::Enter, Modifiers::empty());
    assert_eq!(driver.app_mut().update(|w| w.keys(table.columns())).unwrap(), [C::Score, C::Category]);
    assert_eq!(node(&driver, window, "header-Category"), category);
    assert_eq!(tree(&driver, window).focus(), Some(category));

    let resize = node(&driver, window, "resize");
    let from = window_point(&driver, window, resize, Point::new(5.0, 5.0));
    assert_eq!(runtime(&driver, window).hit_test(from).and_then(|target| target.action).and_then(|target| target.node()), Some(resize));
    let moved = Point::new(from.x - 28.0, from.y);
    pointer_drag(&mut driver, window, from, moved, true);
    let width = |driver: &mut HeadlessDriver| driver.app_mut().update(|w| w.read(|r| table.column_width(r, &C::Category))).unwrap();
    assert_eq!(width(&mut driver), Some(100.0));
    pointer_drag(&mut driver, window, from, moved, false);
    assert_eq!(width(&mut driver), Some(72.0));
    assert_eq!(driver.app_mut().update(|w| w.keys(table.columns())).unwrap(), [C::Score, C::Category]);
    focus(&mut driver, window, "resize");
    key(&mut driver, window, Key::End, Modifiers::empty());
    assert_eq!(width(&mut driver), Some(150.0));

    let from = window_point(&driver, window, category, Point::new(90.0, 5.0));
    let to = window_point(&driver, window, score, Point::new(5.0, 5.0));
    pointer_drag(&mut driver, window, from, to, false);
    assert_eq!(driver.app_mut().update(|w| w.keys(table.columns())).unwrap(), [C::Category, C::Score]);
}

#[test]
fn single_table_sort_takes_only_one_controlled_priority() {
    let mut driver = HeadlessDriver::default();
    let initial = vec![
        Sort { column: C::Category, direction: SortDirection::Ascending },
        Sort { column: C::Score, direction: SortDirection::Descending },
    ];
    let (table, sort) = driver
        .app_mut()
        .build(|b| {
            let sort = b.signal(initial);
            (DataTableState::<u32, _>::new(b, columns(), SortMode::Single, sort, None), sort)
        })
        .unwrap();
    driver
        .app_mut()
        .update(|w| {
            assert_eq!(w.read(|r| table.sort(r)), [Sort { column: C::Category, direction: SortDirection::Ascending }]);
            table.cycle_sort(w, &C::Score, true);
            assert_eq!(w.get(sort), [Sort { column: C::Score, direction: SortDirection::Ascending }]);
            table.cycle_sort(w, &C::Score, false);
            assert_eq!(w.get(sort), [Sort { column: C::Score, direction: SortDirection::Descending }]);
            table.cycle_sort(w, &C::Score, false);
            assert!(w.get(sort).is_empty());
        })
        .unwrap();
}
