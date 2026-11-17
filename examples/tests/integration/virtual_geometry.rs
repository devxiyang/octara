//! A virtual list keeps the reading position by business key while its geometry changes.

use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const ROWS: u32 = 1_000;
const READING: u32 = 500;
const INSET: f32 = 11.0;

#[derive(Clone, Debug, PartialEq)]
struct Item {
    key: u32,
    order: i32,
    height: f32,
    estimate: f32,
    version: u32,
}

impl Item {
    fn new(key: u32) -> Self {
        let height = 40.0 + (key % 4) as f32 * 8.0;
        Self { key, order: key as i32, height, estimate: height, version: 0 }
    }
}

#[derive(Clone, Copy)]
struct Model {
    items: StateList<u32, Item>,
    snapshot: Signal<Vec<(u32, Item)>>,
}

#[component]
fn geometry_row(key: u32, item: ReadSignal<Item>) -> View {
    col()
        .id(format!("row-{key}"))
        .flex_shrink_0()
        .style_with(move |p| p.h(item.with(|item| item.height)))
        .child(text(move || item.with(|item| format!("{}:{}", item.key, item.version))).id(format!("body-{key}")))
        .into()
}

#[component]
fn geometry_list(model: Model, snapshots: bool, follow: bool, gap: f32) -> View {
    let sorted = model.items.sort_by(|item| item.order);
    let render = |key: &u32, item| geometry_row(*key, item);
    let list = col().id("list").size_full().gap_y(gap).overflow_y_scroll().child(if snapshots {
        virtual_each(move || model.snapshot.get(), |item| item.estimate, render)
    } else {
        virtual_each(move || sorted.clone(), |item| item.estimate, render)
    });
    if follow { list.follow_tail().into() } else { list.into() }
}

struct Fixture {
    app: App,
    window: Window,
    model: Model,
    snapshots: bool,
}

impl Fixture {
    fn new(snapshots: bool, follow: bool) -> Self {
        Self::with_gap(snapshots, follow, 0.0)
    }

    fn with_gap(snapshots: bool, follow: bool, gap: f32) -> Self {
        let app = App::new(HeadlessPlatform::default());
        let model = app.run(|| Model {
            items: state_list("items", || (0..ROWS).map(Item::new).collect(), |item| item.key),
            snapshot: state("snapshot", || (0..ROWS).map(|key| (key, Item::new(key))).collect()),
        });
        let window = app
            .open_window(WindowOptions::new("Virtual geometry").size(320.0, 192.0), move || geometry_list(model, snapshots, follow, gap))
            .unwrap();
        Self { app, window, model, snapshots }
    }

    fn scroll(&mut self, y: f32) {
        self.window.dispatch(Event::ScrollTo("list".into(), 0.0, y)).unwrap();
    }

    fn prefix(&self, key: u32) -> f32 {
        self.app.run(|| {
            if self.snapshots {
                self.model.snapshot.with(|items| items.iter().take_while(|(id, _)| *id != key).map(|(_, item)| item.height).sum())
            } else {
                let mut items: Vec<_> = self.model.items.keys().iter().map(|key| self.model.items.item(key).unwrap().get()).collect();
                items.sort_by_key(|item| item.order);
                items.iter().take_while(|item| item.key != key).map(|item| item.height).sum()
            }
        })
    }

    fn read(&mut self, key: u32) -> String {
        self.scroll(self.prefix(key) + INSET);
        close(self.relative_y(key), -INSET, "initial reading position");
        self.window.inspect().find(&format!("row-{key}")).unwrap().target.clone()
    }

    fn insert(&mut self, item: Item) {
        self.app.run(|| {
            if self.snapshots {
                self.model.snapshot.update(|items| {
                    items.push((item.key, item));
                    items.sort_by_key(|(_, item)| item.order);
                });
            } else {
                self.model.items.push(item);
            }
        });
        self.app.settle();
    }

    fn update(&mut self, key: u32, edit: impl FnOnce(&mut Item)) {
        self.app.run(|| {
            if self.snapshots {
                self.model.snapshot.update(|items| {
                    edit(&mut items.iter_mut().find(|(id, _)| *id == key).unwrap().1);
                    items.sort_by_key(|(_, item)| item.order);
                });
            } else {
                self.model.items.update(&key, edit);
            }
        });
        self.app.settle();
    }

    fn retain(&mut self, keep: impl Fn(&Item) -> bool) {
        self.app.run(|| {
            if self.snapshots {
                self.model.snapshot.update(|items| items.retain(|(_, item)| keep(item)));
            } else {
                self.model.items.retain(keep);
            }
        });
        self.app.settle();
    }

    fn relative_y(&self, key: u32) -> f32 {
        let kernel = self.window.kernel();
        let row = kernel.find(&format!("row-{key}")).unwrap_or_else(|| panic!("reading row {key} was unmounted"));
        kernel.rect(row).1 - kernel.content_origin_y(kernel.find("list").unwrap())
    }

    fn assert_reading(&self, key: u32, target: &str) {
        close(self.relative_y(key), -INSET, "the business key and its partial-row offset stay put");
        assert_eq!(
            self.window.inspect().find(&format!("row-{key}")).unwrap().target,
            target,
            "a surviving reading row keeps its allocation"
        );
        self.clean();
    }

    fn clean(&self) {
        let kernel = self.window.kernel();
        let rows: Vec<_> = kernel.child_ids("list").into_iter().filter(|id| id.starts_with("row-")).collect();
        assert!(rows.len() < 28, "only the viewport and overscan are mounted: {rows:?}");
        assert!(kernel.live_nodes() < 96, "measuring does not mount the entire collection");
        drop(kernel);
        assert!(self.window.layout_issues().is_empty(), "{:?}", self.window.layout_issues());
        assert!(self.app.diagnostics().is_empty(), "{:?}", self.app.diagnostics());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.app.shutdown();
    }
}

fn close(actual: f32, expected: f32, context: &str) {
    assert!((actual - expected).abs() <= 0.5, "{context}: expected {expected}, got {actual}");
}

#[test]
fn prepending_variable_rows_keeps_the_reading_business_key_and_offset() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        let target = fixture.read(READING);
        let mut item = Item::new(ROWS + 1);
        item.order = -1;
        item.height = 113.0;
        item.estimate = item.height;
        fixture.insert(item);
        fixture.assert_reading(READING, &target);
    }
}

#[test]
fn prepending_at_the_start_keeps_the_viewport_at_zero() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        let mut item = Item::new(ROWS + 1);
        item.order = -1;
        fixture.insert(item);
        let kernel = fixture.window.kernel();
        close(kernel.scroll_offset(kernel.find("list").unwrap()).1, 0.0, "the start stays at the start");
        drop(kernel);
        close(fixture.relative_y(ROWS + 1), 0.0, "the prepended item appears at the start");
        fixture.clean();
    }
}

#[test]
fn a_reading_offset_inside_a_row_gap_survives_prepend_and_content_changes() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::with_gap(snapshots, false, 10.0);
        // Row 0 occupies [0, 40), its gap [40, 50). Reading at 45 leaves the next
        // business row five pixels below the viewport's top, without entering that row.
        fixture.scroll(45.0);
        close(fixture.relative_y(1), 5.0, "the reader starts halfway through the gap");
        let target = fixture.window.inspect().find("row-1").unwrap().target.clone();
        let mut item = Item::new(ROWS + 1);
        item.order = -1;
        item.height = 40.0;
        item.estimate = item.height;
        fixture.insert(item);
        close(fixture.relative_y(1), 5.0, "prepending preserves the reading offset inside the gap");
        assert_eq!(fixture.window.inspect().find("row-1").unwrap().target, target);
        fixture.update(ROWS + 1, |item| {
            item.height = 120.0;
            item.estimate = item.height;
        });
        close(fixture.relative_y(1), 5.0, "content changes preserve the reading offset inside the gap");
        assert_eq!(fixture.window.inspect().find("row-1").unwrap().target, target);
        fixture.clean();
    }
}

#[test]
fn removing_variable_rows_above_the_viewport_keeps_the_reading_key_and_offset() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        let target = fixture.read(READING);
        fixture.retain(|item| item.key >= 17);
        fixture.assert_reading(READING, &target);
    }
}

#[test]
fn moving_a_row_across_the_viewport_keeps_the_reading_key_and_offset() {
    for snapshots in [false, true] {
        for (key, order) in [(7, 800), (800, 7)] {
            let mut fixture = Fixture::new(snapshots, false);
            let target = fixture.read(READING);
            fixture.update(key, |item| item.order = order);
            fixture.assert_reading(READING, &target);
        }
    }
}

#[test]
fn deleting_the_reading_anchor_uses_the_next_surviving_old_key() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        fixture.read(READING);
        fixture.retain(|item| !(READING..READING + 3).contains(&item.key));
        close(fixture.relative_y(READING + 3), -INSET, "the successor inherits the reading offset");
        fixture.clean();
    }
}

#[test]
fn deleting_all_successors_uses_the_previous_surviving_key_and_clamps_at_the_end() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        fixture.read(READING);
        fixture.retain(|item| item.key < READING);
        let kernel = fixture.window.kernel();
        let list = kernel.find("list").unwrap();
        let previous = kernel.find(&format!("row-{}", READING - 1)).expect("the previous surviving key must be visible");
        close(
            kernel.rect(previous).1 + kernel.rect(previous).3,
            kernel.content_origin_y(list) + kernel.client_size(list).1,
            "the predecessor is revealed at the clamped end",
        );
        close(kernel.scroll_offset(list).1, kernel.scroll_size(list).1 - kernel.client_size(list).1, "the new end clamps the offset");
        drop(kernel);
        fixture.clean();
    }
}

#[test]
fn same_key_height_changes_while_offscreen_update_estimates_without_moving_the_reader() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        let target = fixture.read(READING);
        assert!(!fixture.window.kernel().exists("row-2"));
        fixture.window.reset_counters();
        fixture.update(2, |item| {
            item.height = 152.0;
            item.estimate = item.height;
            item.version += 1;
        });
        fixture.assert_reading(READING, &target);
        assert!(fixture.window.kernel().counters().nodes_created < 48, "the offscreen edit does not build the intervening rows");
        fixture.scroll(0.0);
        let kernel = fixture.window.kernel();
        close(kernel.rect(kernel.find("row-2").unwrap()).3, 152.0, "the remounted row uses its current content");
        assert_eq!(kernel.text_of("body-2").as_deref(), Some("2:1"));
        drop(kernel);
        fixture.read(READING);
        fixture.clean();
    }
}

#[test]
fn resizing_the_reading_row_keeps_its_partial_row_offset_and_allocation() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, false);
        let target = fixture.read(READING);
        fixture.update(READING, |item| {
            item.height = 184.0;
            item.estimate = item.height;
            item.version += 1;
        });
        fixture.assert_reading(READING, &target);
        assert_eq!(fixture.window.kernel().text_of(&format!("body-{READING}")).as_deref(), Some("500:1"));
    }
}

#[test]
fn a_reading_offset_deeper_than_the_estimate_survives_content_and_constraint_invalidation() {
    for snapshots in [false, true] {
        for resize in [false, true] {
            let mut fixture = Fixture::new(snapshots, false);
            fixture.update(READING, |item| {
                item.height = 600.0;
                item.estimate = 40.0;
            });
            // Measure the tall row before entering its middle: a jump may initially use the
            // estimate, but a known reading offset belongs to the row, even beyond that estimate.
            fixture.scroll(fixture.prefix(READING));
            close(row_height(&fixture.window, READING), 600.0, "the reading row has its real height");
            fixture.scroll(fixture.prefix(READING) + 300.0);
            let target = fixture.window.inspect().find(&format!("row-{READING}")).unwrap().target.clone();
            close(fixture.relative_y(READING), -300.0, "the reader starts in the middle of the tall row");
            if resize {
                fixture.window.resize(160.0, 192.0);
            } else {
                fixture.update(2, |item| {
                    item.height = 120.0;
                    item.estimate = item.height;
                });
            }
            close(fixture.relative_y(READING), -300.0, "an estimate cannot clamp a known partial-row reading offset");
            assert_eq!(fixture.window.inspect().find(&format!("row-{READING}")).unwrap().target, target);
            fixture.clean();
        }
    }
}

#[test]
fn shrinking_a_fully_resident_reading_row_clamps_when_the_content_fits_the_viewport() {
    for snapshots in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let initial =
            || (0..3).map(|key| Item { height: if key == 2 { 600.0 } else { 40.0 }, estimate: 40.0, ..Item::new(key) }).collect::<Vec<_>>();
        let model = app.run(|| Model {
            items: state_list("items", initial, |item| item.key),
            snapshot: state("snapshot", || initial().into_iter().map(|item| (item.key, item)).collect()),
        });
        let mut window = app
            .open_window(WindowOptions::new("Resident row shrink").size(320.0, 192.0), move || geometry_list(model, snapshots, false, 0.0))
            .unwrap();
        window.dispatch(Event::ScrollTo("list".into(), 0.0, 380.0)).unwrap();
        {
            let kernel = window.kernel();
            let list = kernel.find("list").unwrap();
            close(
                kernel.rect(kernel.find("row-2").unwrap()).1 - kernel.content_origin_y(list),
                -300.0,
                "the reader is inside the tall last row",
            );
            close(kernel.rect(kernel.find("list-top").unwrap()).3, 0.0, "all earlier rows are resident");
            close(kernel.rect(kernel.find("list-bottom").unwrap()).3, 0.0, "all later rows are resident");
        }
        app.run(|| {
            if snapshots {
                model.snapshot.update(|items| items[2].1.height = 50.0);
            } else {
                model.items.update(&2, |item| item.height = 50.0);
            }
        });
        app.settle();
        {
            let kernel = window.kernel();
            let list = kernel.find("list").unwrap();
            close(kernel.scroll_size(list).1, kernel.client_size(list).1, "the updated content fits the viewport");
            close(kernel.scroll_offset(list).1, 0.0, "restoring an anchor cannot leave the viewport past its new end");
            let last = kernel.rect(kernel.find("row-2").unwrap());
            close(last.1 - kernel.content_origin_y(list), 80.0, "the final row is visible after clamping");
            close(last.3, 50.0, "the final row has its updated height");
            assert!(last.1 + last.3 <= kernel.content_origin_y(list) + kernel.client_size(list).1);
        }
        assert_bounded_and_clean(&app, &window);
        app.shutdown();
    }
}

#[test]
fn an_explicit_scroll_from_a_snapshot_watch_wins_over_the_pending_reading_anchor() {
    let app = App::new(HeadlessPlatform::default());
    let model = app.run(|| Model {
        items: state_list("items", || (0..ROWS).map(Item::new).collect(), |item| item.key),
        snapshot: state("snapshot", || (0..ROWS).map(|key| (key, Item::new(key))).collect()),
    });
    let observed_offset = Rc::new(Cell::new(None));
    let seen_offset = observed_offset.clone();
    let mut window = app
        .open_window(WindowOptions::new("Explicit scroll wins").size(320.0, 192.0), move || {
            let window_id = current_window();
            let seen_offset = seen_offset.clone();
            // Structural observers run before watch callbacks. This request therefore arrives
            // after the list has retained its reading anchor for the edited snapshot.
            watch(
                move || model.snapshot.with(|items| items[2].1.version),
                move |_, previous| {
                    if previous.is_some() {
                        let window = App::current().window(window_id).unwrap();
                        let kernel = window.kernel();
                        seen_offset.set(Some(kernel.scroll_offset(kernel.find("list").unwrap()).1));
                        drop(kernel);
                        assert!(scroll_to("list", 0.0, 0.0));
                    }
                },
            );
            geometry_list(model, true, false, 0.0)
        })
        .unwrap();
    let prefix: f32 = (0..READING).map(|key| Item::new(key).height).sum();
    window.dispatch(Event::ScrollTo("list".into(), 0.0, prefix + INSET)).unwrap();
    app.run(|| {
        model.snapshot.update(|items| {
            items[2].1.height = 152.0;
            items[2].1.estimate = 152.0;
            items[2].1.version += 1;
        });
    });
    app.settle();
    assert!(
        observed_offset.get().expect("the edit triggers the scrolling watch") > prefix,
        "the watch starts from the retained reading position"
    );
    {
        let kernel = window.kernel();
        let list = kernel.find("list").unwrap();
        close(kernel.scroll_offset(list).1, 0.0, "the explicit request supersedes the earlier anchor");
        close(kernel.rect(kernel.find("row-0").unwrap()).1, kernel.content_origin_y(list), "the requested first row is visible");
    }
    assert_bounded_and_clean(&app, &window);
    app.shutdown();
}

#[test]
fn tail_following_wins_over_geometry_anchoring_until_the_reader_scrolls_away() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots, true);
        fixture.insert(Item::new(ROWS));
        let kernel = fixture.window.kernel();
        let list = kernel.find("list").unwrap();
        assert!(kernel.is_following_tail(list));
        close(kernel.scroll_offset(list).1, kernel.scroll_size(list).1 - kernel.client_size(list).1, "appending follows the new end");
        drop(kernel);
        fixture.update(ROWS, |item| {
            item.height = 176.0;
            item.estimate = item.height;
        });
        let kernel = fixture.window.kernel();
        let list = kernel.find("list").unwrap();
        assert!(kernel.is_following_tail(list));
        close(
            kernel.scroll_offset(list).1,
            kernel.scroll_size(list).1 - kernel.client_size(list).1,
            "measuring a taller tail keeps the end in place",
        );
        drop(kernel);
        let target = fixture.read(READING);
        fixture.insert(Item::new(ROWS + 1));
        fixture.assert_reading(READING, &target);
        let kernel = fixture.window.kernel();
        assert!(!kernel.is_following_tail(kernel.find("list").unwrap()), "manual reading stays released from the tail");
    }
}

// Only this row wraps. Its estimate deliberately differs from its measured height, so the
// tests distinguish retiring an obsolete offscreen measurement from eagerly laying it out.
const WRAPPING: u32 = 2;
const TEXT_ESTIMATE: f32 = 32.0;
const FIXED_HEIGHT: f32 = 64.0;
const WORDS: &str = "word word word word word word word word word word word word word word word word";

#[component]
fn wrapping_row(key: u32) -> View {
    if key == WRAPPING {
        col().id(format!("row-{key}")).flex_shrink_0().child(text(WORDS).w_full().text_base().flex_shrink_0()).into()
    } else {
        col().id(format!("row-{key}")).h(FIXED_HEIGHT).flex_shrink_0().child(text(format!("Row {key}")).text_base()).into()
    }
}

#[component]
fn wrapping_list(items: StateList<u32, u32>, snapshot: Signal<Vec<(u32, u32)>>, snapshots: bool) -> View {
    let estimate = |key: &u32| if *key == WRAPPING { TEXT_ESTIMATE } else { FIXED_HEIGHT };
    let render = |key: &u32, _| wrapping_row(*key);
    col()
        .id("list")
        .size_full()
        .overflow_y_scroll()
        .child(if snapshots {
            virtual_each(move || snapshot.get(), estimate, render)
        } else {
            virtual_each(move || items.read_only(), estimate, render)
        })
        .into()
}

fn row_height(window: &Window, key: u32) -> f32 {
    let kernel = window.kernel();
    kernel.rect(kernel.find(&format!("row-{key}")).unwrap()).3
}

fn assert_bounded_and_clean(app: &App, window: &Window) {
    let kernel = window.kernel();
    assert!(kernel.child_ids("list").into_iter().filter(|id| id.starts_with("row-")).count() < 28);
    assert!(kernel.live_nodes() < 96, "offscreen geometry uses estimates instead of mounted rows");
    drop(kernel);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

fn check_wrapping_invalidation(snapshots: bool, theme_change: bool) {
    let app = App::with_config(
        HeadlessPlatform::default(),
        octara::app::AppConfig { text_engine: Box::new(octara_text::MonoEngine), ..Default::default() },
    );
    let (items, snapshot) = app
        .run(|| (state_list("items", || (0..ROWS).collect(), |key| *key), state("snapshot", || (0..ROWS).map(|key| (key, key)).collect())));
    let mut window = app
        .open_window(WindowOptions::new("Wrapping geometry").size(320.0, 192.0), move || wrapping_list(items, snapshot, snapshots))
        .unwrap();
    let initial_height = row_height(&window, WRAPPING);
    assert!(initial_height > TEXT_ESTIMATE, "the first layout replaced the estimate");
    let prefix = (READING - 1) as f32 * FIXED_HEIGHT + initial_height;
    window.dispatch(Event::ScrollTo("list".into(), 0.0, prefix + INSET)).unwrap();
    assert!(!window.kernel().exists(&format!("row-{WRAPPING}")));
    let reading = window.inspect().find(&format!("row-{READING}")).unwrap().target.clone();
    window.reset_counters();
    if theme_change {
        app.set_theme(Theme::light().text(TextStep::Base, 24.0));
    } else {
        window.resize(160.0, 192.0);
    }
    {
        let kernel = window.kernel();
        let list = kernel.find("list").unwrap();
        let row = kernel.find(&format!("row-{READING}")).expect("the reading row survives changed constraints");
        close(kernel.rect(row).1 - kernel.content_origin_y(list), -INSET, "width and font changes keep the reader in place");
        // Unknown offscreen text can use an estimate again; retaining its old measured height
        // as a current measurement would make this extent differ by initial_height - 32.
        close(
            kernel.scroll_size(list).1,
            (ROWS - 1) as f32 * FIXED_HEIGHT + TEXT_ESTIMATE,
            "offscreen measurements from old constraints are retired",
        );
    }
    assert_eq!(window.inspect().find(&format!("row-{READING}")).unwrap().target, reading);
    assert!(window.kernel().counters().nodes_created < 64, "constraint changes do not build the gap");
    assert_bounded_and_clean(&app, &window);
    window.dispatch(Event::ScrollTo("list".into(), 0.0, 0.0)).unwrap();
    let updated_height = row_height(&window, WRAPPING);
    assert!(updated_height > initial_height, "the revisited text reflows with its current width and font size");
    let prefix = (READING - 1) as f32 * FIXED_HEIGHT + updated_height;
    window.dispatch(Event::ScrollTo("list".into(), 0.0, prefix + INSET)).unwrap();
    {
        let kernel = window.kernel();
        let list = kernel.find("list").unwrap();
        let row = kernel.find(&format!("row-{READING}")).expect("the viewport converges after remeasurement");
        close(kernel.rect(row).1 - kernel.content_origin_y(list), -INSET, "remeasuring offscreen text updates its logical extent");
    }
    assert_bounded_and_clean(&app, &window);
    app.shutdown();
}

#[test]
fn width_changes_retire_offscreen_measurements_and_remeasure_with_bounded_mounts() {
    for snapshots in [false, true] {
        check_wrapping_invalidation(snapshots, false);
    }
}

#[test]
fn font_metric_changes_retire_offscreen_measurements_and_remeasure_with_bounded_mounts() {
    for snapshots in [false, true] {
        check_wrapping_invalidation(snapshots, true);
    }
}

struct CountedItem {
    item: Item,
    reads: Rc<Cell<usize>>,
}

impl Clone for CountedItem {
    fn clone(&self) -> Self {
        self.reads.set(self.reads.get() + 1);
        Self { item: self.item.clone(), reads: self.reads.clone() }
    }
}

#[component]
fn counted_list(items: StateList<u32, CountedItem>, second: StateList<u32, CountedItem>, use_second: Signal<bool>) -> View {
    col()
        .id("list")
        .size_full()
        .overflow_y_scroll()
        .child(virtual_each(
            move || if use_second.get() { second.read_only() } else { items.read_only() },
            |item| item.item.estimate,
            |key, item| col().id(format!("row-{key}")).flex_shrink_0().style_with(move |p| p.h(item.with(|item| item.item.height))).into(),
        ))
        .into()
}

#[test]
fn an_offscreen_content_edit_reads_only_changed_items_and_retires_observers_on_switch_or_disposal() {
    let app = App::new(HeadlessPlatform::default());
    let reads = Rc::new(Cell::new(0));
    let (items, second, use_second, shown) = app.run(|| {
        (
            state_list(
                "items",
                || (0..ROWS).map(|key| CountedItem { item: Item::new(key), reads: reads.clone() }).collect(),
                |item| item.item.key,
            ),
            state_list(
                "second",
                || (0..ROWS).map(|key| CountedItem { item: Item::new(key), reads: reads.clone() }).collect(),
                |item| item.item.key,
            ),
            state("use_second", || false),
            state("shown", || true),
        )
    });
    let mut window = app
        .open_window(WindowOptions::new("Dirty geometry").size(320.0, 192.0), move || {
            col().size_full().child(show(move || shown.get(), move || counted_list(items, second, use_second))).into()
        })
        .unwrap();
    let prefix: f32 = (0..READING).map(|key| Item::new(key).height).sum();
    window.dispatch(Event::ScrollTo("list".into(), 0.0, prefix + INSET)).unwrap();
    reads.set(0);
    app.run(|| {
        items.update(&2, |item| {
            item.item.height = 152.0;
            item.item.estimate = item.item.height;
        })
    });
    app.settle();
    assert!(reads.get() <= 8, "one dirty key should not reread all {ROWS} items: {} reads", reads.get());
    {
        let kernel = window.kernel();
        let list = kernel.find("list").unwrap();
        close(
            kernel.rect(kernel.find(&format!("row-{READING}")).unwrap()).1 - kernel.content_origin_y(list),
            -INSET,
            "the dirty key updates geometry around the current reader",
        );
    }
    app.run(|| {
        items.update(&2, |item| item.item.estimate = 184.0);
        use_second.set(true);
    });
    app.settle();
    reads.set(0);
    app.run(|| items.update(&2, |item| item.item.estimate = 200.0));
    app.settle();
    assert_eq!(reads.get(), 2, "the retired source is only read by its own update, never by the virtual list");
    assert_bounded_and_clean(&app, &window);
    app.run(|| {
        second.update(&2, |item| item.item.estimate = 184.0);
        shown.set(false);
    });
    app.settle();
    assert!(!window.kernel().exists("list"));
    reads.set(0);
    app.run(|| second.update(&2, |item| item.item.estimate = 200.0));
    app.settle();
    assert_eq!(reads.get(), 2, "disposal removes the item observer even with a dirty key pending");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}
