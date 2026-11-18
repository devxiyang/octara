//! Virtual rows keep active editors and pointer deliveries resident, without retaining the gap.

use octara::app::{Phase, PointerEvent};
use octara::inspect::{self, Delivery};
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const ROW_HEIGHT: f32 = 64.0;
const ROWS: u32 = 10_000;
const FAR: f32 = 5_000.0 * ROW_HEIGHT;

#[derive(Clone, Copy)]
struct Model {
    first: StateList<u32, u32>,
    second: StateList<u32, u32>,
    snapshot: Signal<Vec<(u32, u32)>>,
    use_second: Signal<bool>,
    shown: Signal<bool>,
    scroll_during_end: Signal<bool>,
}

type Drags = Rc<RefCell<Vec<(u32, DragPhase)>>>;

#[ui]
fn interaction_row(key: u32, item: ReadSignal<u32>, model: Model, drags: Drags, disposed: Rc<RefCell<Vec<u32>>>) -> El {
    let draft = state("draft", || "A😀bcZ".to_owned());
    let window_id = current_window();
    on_cleanup(move || disposed.borrow_mut().push(key));
    row()
        .id(format!("row-{key}"))
        .h(ROW_HEIGHT)
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .child(
            text_input()
                .id(format!("editor-{key}"))
                .label(format!("Editor {key}"))
                .w(240)
                .h(40)
                .value(draft)
                .on_input(move |v| draft.set(v)),
        )
        .child(
            el().id(format!("drag-{key}"))
                .label(format!("Drag {key}"))
                .w(120)
                .h(40)
                .preserve_focus_on_press(true)
                .child(text(move || format!("Drag {}", item.get())))
                .on_drag(move |drag| {
                    if drag.phase == DragPhase::End && model.scroll_during_end.get() {
                        let mut window = App::current().window(window_id).unwrap();
                        window.dispatch(Event::ScrollTo("list".into(), 0.0, FAR)).unwrap();
                        assert!(window.kernel().exists(&format!("drag-{key}")), "the final callback still owns its allocation");
                        assert_eq!(item.get(), key, "the final callback still owns its live row input");
                    }
                    drags.borrow_mut().push((key, drag.phase));
                })
                .on_long_press(move || {
                    let mut window = App::current().window(window_id).unwrap();
                    window.dispatch(Event::ScrollTo("list".into(), 0.0, FAR)).unwrap();
                    assert!(window.kernel().exists(&format!("drag-{key}")), "long-press delivery still owns its allocation");
                    assert_eq!(item.get(), key);
                }),
        )
}

#[ui]
fn interaction_list(model: Model, snapshots: bool, gap: f32, drags: Drags, disposed: Rc<RefCell<Vec<u32>>>) -> View {
    let render = move |key: &u32, item| interaction_row(*key, item, model, drags.clone(), disposed.clone());
    col()
        .id("list")
        .h(192)
        .gap_y(gap)
        .overflow_y_scroll()
        .child(if snapshots {
            virtual_each(move || model.snapshot.get(), |_| ROW_HEIGHT, render)
        } else {
            virtual_each(
                move || if model.use_second.get() { model.second.read_only() } else { model.first.read_only() },
                |_| ROW_HEIGHT,
                render,
            )
        })
        .into()
}

struct Fixture {
    app: App,
    window: Window,
    model: Model,
    drags: Drags,
    disposed: Rc<RefCell<Vec<u32>>>,
}

impl Fixture {
    fn new(snapshots: bool) -> Self {
        Self::with_gap(snapshots, 0.0)
    }

    fn with_gap(snapshots: bool, gap: f32) -> Self {
        let app = App::new(HeadlessPlatform::default());
        let model = app.run(|| Model {
            first: state_list("first", || (0..ROWS).collect(), |key| *key),
            second: state_list("second", || (0..ROWS).collect(), |key| *key),
            snapshot: state("snapshot", || if snapshots { (0..ROWS).map(|key| (key, key)).collect() } else { Vec::new() }),
            use_second: state("use_second", || false),
            shown: state("shown", || true),
            scroll_during_end: state("scroll_during_end", || false),
        });
        let drags = Drags::default();
        let disposed = Rc::new(RefCell::new(Vec::new()));
        let (seen_drags, seen_disposed) = (drags.clone(), disposed.clone());
        let window = app
            .open_window(WindowOptions::new("Virtual interaction").size(640.0, 260.0), move || {
                let (drags, disposed) = (seen_drags.clone(), seen_disposed.clone());
                col()
                    .size_full()
                    .child(el().id("outside").role(Role::Button).label("Outside list").tab_index(0).h(40).on_press(|| {}))
                    .child(show(
                        move || model.shown.get(),
                        move || interaction_list(model, snapshots, gap, drags.clone(), disposed.clone()),
                    ))
            })
            .unwrap();
        Self { app, window, model, drags, disposed }
    }

    fn scroll(&mut self, y: f32) {
        self.window.dispatch(Event::ScrollTo("list".into(), 0.0, y)).unwrap();
    }

    fn focus(&mut self) {
        self.window.dispatch(Event::Focus("editor-0".into())).unwrap();
    }

    fn compose(&mut self) {
        self.focus();
        self.window.set_selection(7, 5);
        self.window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 4)) });
    }

    fn start_drag(&mut self, key: u32) -> (f32, f32) {
        let (x, y) = center(&self.window, &format!("drag-{key}"));
        self.window.pointer_down(x, y);
        self.window.pointer_move(x + 12.0, y);
        assert_eq!(self.drags.borrow().last(), Some(&(key, DragPhase::Start)));
        (x + 12.0, y)
    }

    fn clean(&self) {
        assert!(self.window.layout_issues().is_empty(), "{:?}", self.window.layout_issues());
        assert!(self.app.diagnostics().is_empty(), "{:?}", self.app.diagnostics());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.app.shutdown();
    }
}

fn center(window: &Window, id: &str) -> (f32, f32) {
    let kernel = window.kernel();
    let (x, y, width, height) = kernel.rect(kernel.find(id).unwrap());
    (x + width / 2.0, y + height / 2.0)
}

fn target(window: &Window, id: &str) -> String {
    window.inspect().find(id).unwrap().target.clone()
}

fn row_ids(window: &Window) -> Vec<String> {
    window.kernel().child_ids("list").into_iter().filter(|id| id.starts_with("row-")).collect()
}

fn assert_sparse_geometry(window: &Window, keys: &[u32], removed: u32) {
    let kernel = window.kernel();
    let list = kernel.find("list").unwrap();
    let origin = kernel.content_origin_y(list);
    let offset = kernel.scroll_offset(list).1;
    assert_eq!(kernel.scroll_size(list).1, (ROWS - removed) as f32 * ROW_HEIGHT);
    for &key in keys {
        let rect = kernel.rect(kernel.find(&format!("row-{key}")).unwrap());
        assert_eq!(rect.1, origin + (key - removed) as f32 * ROW_HEIGHT - offset, "row {key} keeps its logical position");
        assert_eq!(rect.3, ROW_HEIGHT);
    }
    let ids = row_ids(window);
    assert!(ids.len() < 24, "only the viewport, overscan, and active rows reside: {ids:?}");
    assert!(kernel.live_nodes() < 150, "gaps use spacers, not thousands of mounted rows");
    let indices: Vec<u32> = ids.iter().map(|id| id.trim_start_matches("row-").parse().unwrap()).collect();
    assert!(indices.windows(2).all(|pair| pair[0] < pair[1]), "resident rows stay in data order: {indices:?}");
}

fn stale_type(window: &mut Window, id: &str, target: &str) {
    let reply = inspect::handle_line(window, &format!(r#"{{"op":"type","id":"{id}","text":"stale","target":"{target}"}}"#));
    assert!(!reply.ok, "{reply:?}");
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"), "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Refused), "{reply:?}");
}

#[test]
fn composing_editor_keeps_identity_selection_caret_and_undo_across_far_scrolls() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots);
        fixture.focus();
        fixture.window.set_selection(5, 7);
        fixture.window.type_text("x");
        fixture.window.set_selection(6, 5);
        fixture.window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 4)) });
        let before = fixture.window.text_field().unwrap();
        let allocation = target(&fixture.window, "editor-0");
        fixture.window.reset_counters();

        for offset in [FAR, FAR + 1_000.0 * ROW_HEIGHT, FAR] {
            fixture.scroll(offset);
            assert_eq!(fixture.window.inspect().focus.as_deref(), Some("editor-0"));
            assert_eq!(target(&fixture.window, "editor-0"), allocation);
            let after = fixture.window.text_field().expect("the offscreen editor still owns text input");
            assert_eq!(after.text, before.text);
            assert_eq!(after.selection, before.selection);
            assert_eq!(after.reversed, before.reversed);
            assert_eq!(after.composition, before.composition);
            assert_eq!(after.composition_selection, before.composition_selection);
            assert_eq!(after.caret.0, before.caret.0);
            assert_eq!(after.caret.1, before.caret.1 - offset);
            assert_sparse_geometry(&fixture.window, &[0, (offset / ROW_HEIGHT) as u32], 0);
            assert!(!fixture.window.kernel().exists("row-100"), "the interval between editor and viewport stays unmounted");
        }

        fixture.scroll(0.0);
        assert_eq!(fixture.window.text_field().unwrap(), before);
        fixture.window.event(InputEvent::Preedit { text: String::new(), selection: None });
        fixture.window.type_text("你好");
        assert_eq!(fixture.window.kernel().value_of("editor-0").as_deref(), Some("A😀你好Z"));
        fixture.window.press_key("mod+z").unwrap();
        assert_eq!(fixture.window.kernel().value_of("editor-0").as_deref(), Some("A😀xZ"));
        assert_eq!(target(&fixture.window, "editor-0"), allocation);
        fixture.clean();
    }
}

#[test]
fn editing_an_offscreen_resident_editor_reveals_its_caret_without_rebuilding_it() {
    let mut fixture = Fixture::new(false);
    fixture.focus();
    fixture.window.set_selection(8, 8);
    let allocation = target(&fixture.window, "editor-0");
    fixture.scroll(FAR);
    fixture.window.type_text("!");
    assert_eq!(fixture.window.kernel().value_of("editor-0").as_deref(), Some("A😀bcZ!"));
    assert_eq!(target(&fixture.window, "editor-0"), allocation);
    let kernel = fixture.window.kernel();
    let list = kernel.find("list").unwrap();
    let caret = fixture.window.caret().unwrap();
    assert!(caret.1 >= kernel.content_origin_y(list));
    assert!(caret.1 + caret.3 <= kernel.content_origin_y(list) + kernel.client_size(list).1);
    drop(kernel);
    assert!(row_ids(&fixture.window).len() < 24);
    fixture.clean();
}

#[test]
fn focus_and_pointer_capture_pin_only_their_rows_and_release_in_the_same_frame() {
    let mut fixture = Fixture::new(false);
    fixture.scroll(FAR);
    let baseline_rows = row_ids(&fixture.window).len();
    let baseline_nodes = fixture.window.kernel().live_nodes();
    let baseline_reactive = fixture.app.work().reactive_nodes;
    fixture.scroll(0.0);
    fixture.compose();
    let editor = target(&fixture.window, "editor-0");
    let drag = target(&fixture.window, "drag-1");
    let (x, y) = fixture.start_drag(1);
    fixture.window.reset_counters();
    fixture.scroll(FAR);

    assert_eq!(target(&fixture.window, "editor-0"), editor);
    assert_eq!(target(&fixture.window, "drag-1"), drag);
    assert_eq!(row_ids(&fixture.window).len(), baseline_rows + 2);
    assert!(fixture.window.kernel().counters().nodes_created < 100, "a jump must not build the gap");
    assert_sparse_geometry(&fixture.window, &[0, 1, 5_000], 0);
    fixture.window.pointer_move(x + 30.0, y);
    assert_eq!(fixture.drags.borrow().last(), Some(&(1, DragPhase::Move)));
    fixture.window.pointer_up(x + 30.0, y);
    assert_eq!(fixture.drags.borrow().last(), Some(&(1, DragPhase::End)));
    assert!(fixture.window.held_pointer().is_none());
    assert!(!fixture.window.kernel().exists("row-1"));
    assert!(fixture.window.kernel().exists("row-0"));
    assert_eq!(row_ids(&fixture.window).len(), baseline_rows + 1);

    fixture.window.dispatch(Event::Blur).unwrap();
    assert!(!fixture.window.kernel().exists("row-0"));
    assert_eq!(row_ids(&fixture.window).len(), baseline_rows);
    assert_eq!(fixture.window.kernel().live_nodes(), baseline_nodes);
    assert_eq!(fixture.app.work().reactive_nodes, baseline_reactive);
    assert!(fixture.window.text_field().is_none());
    fixture.clean();
}

#[test]
fn pointer_cancel_and_window_deactivation_release_capture_but_keep_editor_focus() {
    for deactivate in [false, true] {
        let mut fixture = Fixture::new(false);
        fixture.compose();
        let editor = fixture.window.text_field().unwrap();
        let (x, y) = fixture.start_drag(1);
        fixture.scroll(FAR);
        if deactivate {
            fixture.window.event(InputEvent::Active(false));
        } else {
            fixture.window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Cancel, x, y)));
        }
        assert_eq!(fixture.drags.borrow().last(), Some(&(1, DragPhase::Cancel)));
        assert!(fixture.window.held_pointer().is_none());
        assert!(!fixture.window.kernel().exists("row-1"));
        assert_eq!(fixture.window.inspect().focus.as_deref(), Some("editor-0"));
        assert_eq!(fixture.window.text_field().unwrap().composition, editor.composition);
        let count = fixture.drags.borrow().len();
        fixture.window.pointer_up(x, y);
        assert_eq!(fixture.drags.borrow().len(), count, "late release cannot reenter a cancelled delivery");
        fixture.clean();
    }
}

#[test]
fn removing_real_data_retires_pins_and_stale_targets_even_if_the_key_is_reinserted() {
    for snapshots in [false, true] {
        let mut fixture = Fixture::new(snapshots);
        fixture.compose();
        let editor = target(&fixture.window, "editor-0");
        let (x, y) = fixture.start_drag(1);
        fixture.scroll(FAR);
        fixture.app.run(|| {
            if snapshots {
                fixture.model.snapshot.update(|items| items.retain(|(key, _)| *key > 1));
            } else {
                fixture.model.first.remove(&0);
                fixture.model.first.remove(&1);
            }
        });
        fixture.app.settle();
        assert!(!fixture.window.kernel().exists("row-0"));
        assert!(!fixture.window.kernel().exists("row-1"));
        assert!(fixture.window.inspect().focus.is_none());
        assert!(fixture.window.text_field().is_none());
        assert!(fixture.window.held_pointer().is_none());
        assert!(fixture.disposed.borrow().contains(&0) && fixture.disposed.borrow().contains(&1));
        let count = fixture.drags.borrow().len();
        fixture.window.pointer_move(x + 30.0, y);
        fixture.window.pointer_up(x + 30.0, y);
        fixture.window.type_text("late commit");
        assert_eq!(fixture.drags.borrow().len(), count, "removed handlers cannot receive late drag events");
        stale_type(&mut fixture.window, "editor-0", &editor);

        fixture.app.run(|| {
            if snapshots {
                fixture.model.snapshot.update(|items| items.insert(0, (0, 0)));
            } else {
                fixture.model.first.insert(0, 0);
            }
        });
        fixture.app.settle();
        fixture.scroll(0.0);
        assert_ne!(target(&fixture.window, "editor-0"), editor);
        stale_type(&mut fixture.window, "editor-0", &editor);
        fixture.focus();
        assert_eq!(fixture.window.text_field().unwrap().composition, "");
        assert_eq!(fixture.window.kernel().value_of("editor-0").as_deref(), Some("A😀bcZ"));
        fixture.clean();
    }
}

#[test]
fn source_switches_keep_surviving_interactions_and_retire_absent_keys() {
    let mut fixture = Fixture::new(false);
    fixture.compose();
    let editor = target(&fixture.window, "editor-0");
    let drag = target(&fixture.window, "drag-1");
    let (x, y) = fixture.start_drag(1);
    fixture.scroll(FAR);
    fixture.app.run(|| fixture.model.use_second.set(true));
    fixture.app.settle();
    assert_eq!(target(&fixture.window, "editor-0"), editor);
    assert_eq!(target(&fixture.window, "drag-1"), drag);
    assert_eq!(fixture.window.text_field().unwrap().composition, "nihao");
    fixture.window.pointer_move(x + 30.0, y);
    assert_eq!(fixture.drags.borrow().last(), Some(&(1, DragPhase::Move)));
    fixture.app.run(|| {
        fixture.model.first.remove(&0);
        fixture.model.first.remove(&1);
        fixture.model.use_second.set(false);
    });
    fixture.app.settle();
    assert!(!fixture.window.kernel().exists("row-0"));
    assert!(!fixture.window.kernel().exists("row-1"));
    assert!(fixture.window.text_field().is_none());
    assert!(fixture.window.held_pointer().is_none());
    stale_type(&mut fixture.window, "editor-0", &editor);
    fixture.clean();
}

#[test]
fn the_final_drag_callback_keeps_its_row_input_alive_during_a_nested_scroll() {
    let mut fixture = Fixture::new(false);
    fixture.app.run(|| fixture.model.scroll_during_end.set(true));
    let (x, y) = fixture.start_drag(1);
    fixture.window.pointer_up(x, y);
    assert_eq!(fixture.drags.borrow().last(), Some(&(1, DragPhase::End)));
    assert!(!fixture.window.kernel().exists("row-1"), "the pin is released when final delivery returns");
    fixture.clean();
}

#[test]
fn branch_disposal_and_window_close_cancel_all_resident_interactions() {
    for close in [false, true] {
        let mut fixture = Fixture::new(false);
        fixture.compose();
        let editor = target(&fixture.window, "editor-0");
        let (x, y) = fixture.start_drag(1);
        fixture.scroll(FAR);
        if close {
            fixture.app.close_window(fixture.window.id());
            assert!(fixture.window.is_closed());
            assert!(fixture.app.windows().is_empty());
        } else {
            fixture.app.run(|| fixture.model.shown.set(false));
            fixture.app.settle();
            assert!(!fixture.window.kernel().exists("list"));
        }
        assert!(fixture.window.held_pointer().is_none());
        assert!(fixture.window.text_field().is_none());
        assert!(fixture.disposed.borrow().contains(&0) && fixture.disposed.borrow().contains(&1));
        let count = fixture.drags.borrow().len();
        fixture.window.pointer_move(x + 30.0, y);
        fixture.window.pointer_up(x + 30.0, y);
        fixture.window.event(InputEvent::Text("late commit".into()));
        assert_eq!(fixture.drags.borrow().len(), count);
        stale_type(&mut fixture.window, "editor-0", &editor);
        if !close {
            fixture.app.run(|| fixture.model.shown.set(true));
            fixture.app.settle();
            fixture.scroll(0.0);
            assert_ne!(target(&fixture.window, "editor-0"), editor);
            fixture.focus();
            assert_eq!(fixture.window.text_field().unwrap().composition, "");
            fixture.clean();
        }
    }
}

#[test]
fn an_input_callback_can_remove_its_row_before_the_queued_caret_reveal() {
    let removed = Rc::new(Cell::new(false));
    let seen = removed.clone();
    let mut window = Window::headless(move || {
        let items = state_list("items", || (0..100u32).collect(), |key| *key);
        let seen = seen.clone();
        col().size_full().id("list").overflow_y_scroll().child(virtual_each(
            move || items.read_only(),
            |_| ROW_HEIGHT,
            move |key, _| {
                let key = *key;
                let seen = seen.clone();
                text_input().id(format!("editor-{key}")).label(format!("Editor {key}")).h(ROW_HEIGHT).flex_shrink_0().on_input(move |_| {
                    items.remove(&key);
                    seen.set(true);
                })
            },
        ))
    });
    window.dispatch(Event::Focus("editor-0".into())).unwrap();
    window.type_text("remove me");
    assert!(removed.get());
    assert!(!window.kernel().exists("editor-0"));
    assert!(window.text_field().is_none());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    window.app().shutdown();
}

#[test]
fn deleting_a_pending_hit_cancels_the_pointer_even_when_its_ancestor_target_survives() {
    let app = App::new(HeadlessPlatform::default());
    let items = app.run(|| state_list("items", || (0..ROWS).collect(), |key| *key));
    let hits = Rc::new(Cell::new(0));
    let seen = hits.clone();
    let mut window = app
        .open_window(WindowOptions::new("Ancestor pointer target").size(640.0, 192.0), move || {
            let seen = seen.clone();
            col().size_full().id("list").overflow_y_scroll().on_press(move || seen.set(seen.get() + 1)).child(virtual_each(
                move || items.read_only(),
                |_| ROW_HEIGHT,
                |key, _| {
                    col()
                        .id(format!("row-{key}"))
                        .h(ROW_HEIGHT)
                        .flex_shrink_0()
                        .child(text(format!("Hit row {key}")).id(format!("hit-{key}")))
                },
            ))
        })
        .unwrap();
    let (x, y) = center(&window, "hit-0");
    window.pointer_down(x, y);
    window.dispatch(Event::ScrollTo("list".into(), 0.0, FAR)).unwrap();
    assert!(window.kernel().exists("row-0"), "the pending hit also owns its row");
    app.run(|| items.remove(&0));
    app.settle();
    assert!(window.kernel().exists("list"));
    assert!(!window.kernel().exists("row-0"));
    assert!(window.held_pointer().is_none());
    window.pointer_up(x, y);
    assert_eq!(hits.get(), 0, "a late release cannot activate the surviving ancestor");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[ui]
fn nested_virtual_row(outer: u32, editors: bool) -> El {
    col().id(format!("outer-{outer}")).h(192.0).flex_shrink_0().child(
        col().id(format!("inner-{outer}")).h(128.0).overflow_y_scroll().child(virtual_each(
            || (0..1_000u32).map(|key| (key, key)).collect::<Vec<_>>(),
            |_| 32.0,
            move |inner, _| {
                let content = if editors {
                    text_input().id(format!("nested-editor-{outer}-{inner}")).label("Nested editor").h(32.0)
                } else {
                    text(format!("Nested row {outer}:{inner}"))
                };
                col().id(format!("inner-row-{outer}-{inner}")).h(32.0).flex_shrink_0().on_press(|| {}).child(content)
            },
        )),
    )
}

#[ui]
fn nested_virtual_lists(editors: bool) -> View {
    col()
        .size_full()
        .child(col().id("outer-list").h(192.0).overflow_y_scroll().child(virtual_each(
            || (0..1_000u32).map(|key| (key, key)).collect::<Vec<_>>(),
            |_| 192.0,
            move |outer, _| nested_virtual_row(*outer, editors),
        )))
        .into()
}

#[test]
fn nested_virtual_lists_keep_every_ancestor_row_of_the_focused_editor() {
    let mut window = Window::headless(|| nested_virtual_lists(true));
    window.dispatch(Event::Focus("nested-editor-0-0".into())).unwrap();
    window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((2, 4)) });
    let editor = target(&window, "nested-editor-0-0");
    window.dispatch(Event::ScrollTo("inner-0".into(), 0.0, 500.0 * 32.0)).unwrap();
    window.dispatch(Event::ScrollTo("outer-list".into(), 0.0, 500.0 * 192.0)).unwrap();
    assert_eq!(target(&window, "nested-editor-0-0"), editor);
    assert!(window.kernel().exists("outer-0"));
    assert!(!window.kernel().exists("outer-100"));
    assert!(!window.kernel().exists("inner-row-0-100"));
    assert_eq!(window.text_field().unwrap().composition, "nihao");
    window.dispatch(Event::Blur).unwrap();
    assert!(!window.kernel().exists("outer-0"));
    assert!(window.text_field().is_none());
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    window.app().shutdown();
}

#[test]
fn a_touch_pan_retains_its_nested_scroller_but_can_evict_the_original_hit_row() {
    let mut window = Window::headless(|| nested_virtual_lists(false));
    let (x, y) = center(&window, "inner-row-0-0");
    window.event(InputEvent::Pointer(PointerEvent::touch(7, Phase::Down, x, y)));
    window.event(InputEvent::Pointer(PointerEvent::touch(7, Phase::Move, x, y - 20.0)));
    window.dispatch(Event::ScrollTo("inner-0".into(), 0.0, 500.0 * 32.0)).unwrap();
    assert!(!window.kernel().exists("inner-row-0-0"), "a panning scroller no longer needs its original hit");
    window.dispatch(Event::ScrollTo("outer-list".into(), 0.0, 500.0 * 192.0)).unwrap();
    assert!(window.kernel().exists("outer-0"), "the nested scroller remains in its resident outer row");
    assert!(window.held_pointer().is_some());
    let before = {
        let kernel = window.kernel();
        kernel.scroll_offset(kernel.find("inner-0").unwrap()).1
    };
    window.event(InputEvent::Pointer(PointerEvent::touch(7, Phase::Move, x, y - 40.0)));
    let kernel = window.kernel();
    assert!(kernel.scroll_offset(kernel.find("inner-0").unwrap()).1 > before, "the captured pan continues offscreen");
    drop(kernel);
    window.event(InputEvent::Pointer(PointerEvent::touch(7, Phase::Up, x, y - 40.0)));
    assert!(window.held_pointer().is_none());
    assert!(!window.kernel().exists("outer-0"));
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    window.app().shutdown();
}

#[test]
fn a_drag_handler_that_removes_its_row_cannot_restore_a_retired_capture() {
    let drags = Rc::new(RefCell::new(Vec::new()));
    let seen = drags.clone();
    let mut window = Window::headless(move || {
        let items = state_list("items", || (0..1_000u32).collect(), |key| *key);
        let window_id = current_window();
        let seen = seen.clone();
        col().size_full().id("list").overflow_y_scroll().child(virtual_each(
            move || items.read_only(),
            |_| ROW_HEIGHT,
            move |key, _| {
                let key = *key;
                let seen = seen.clone();
                el().id(format!("row-{key}")).label(format!("Drag {key}")).h(ROW_HEIGHT).flex_shrink_0().on_drag(move |drag| {
                    seen.borrow_mut().push(drag.phase);
                    if drag.phase == DragPhase::Start {
                        items.remove(&key);
                        let mut window = App::current().window(window_id).unwrap();
                        window.dispatch(Event::ScrollTo("list".into(), 0.0, 10_000.0)).unwrap();
                    }
                })
            },
        ))
    });
    let (x, y) = center(&window, "row-0");
    window.pointer_down(x, y);
    window.pointer_move(x + 12.0, y);
    assert!(!window.kernel().exists("row-0"));
    assert!(window.held_pointer().is_none(), "the local pointer recognizer cannot overwrite retirement cleanup");
    let count = drags.borrow().len();
    window.pointer_move(x + 24.0, y);
    window.pointer_up(x + 24.0, y);
    assert_eq!(drags.borrow().len(), count);
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    window.app().shutdown();
}

#[test]
fn sparse_pins_preserve_row_gaps_visible_geometry_and_total_scroll_extent() {
    let gap = 8.0;
    let mut control = Fixture::with_gap(false, gap);
    control.scroll(FAR);
    let control_rows = row_ids(&control.window);
    let control_nodes = control.window.kernel().live_nodes();
    let extent = {
        let kernel = control.window.kernel();
        kernel.scroll_size(kernel.find("list").unwrap()).1
    };
    assert_eq!(extent, ROWS as f32 * ROW_HEIGHT + (ROWS - 1) as f32 * gap);

    let mut pinned = Fixture::with_gap(false, gap);
    pinned.compose();
    pinned.scroll(FAR);
    assert!(pinned.window.kernel().exists("row-0"));
    assert_eq!(row_ids(&pinned.window).len(), control_rows.len() + 1);
    {
        let kernel = pinned.window.kernel();
        let reference = control.window.kernel();
        assert_eq!(kernel.scroll_size(kernel.find("list").unwrap()).1, extent);
        assert_eq!(kernel.scroll_offset(kernel.find("list").unwrap()), reference.scroll_offset(reference.find("list").unwrap()));
        for id in &control_rows {
            assert_eq!(kernel.rect(kernel.find(id).unwrap()), reference.rect(reference.find(id).unwrap()), "pinning must not move {id}");
        }
    }
    assert!(!pinned.window.kernel().exists("row-100"));
    pinned.window.dispatch(Event::Blur).unwrap();
    assert_eq!(row_ids(&pinned.window), control_rows);
    assert_eq!(pinned.window.kernel().live_nodes(), control_nodes);
    control.clean();
    pinned.clean();
}

#[test]
fn a_long_press_callback_keeps_its_row_input_alive_during_a_nested_scroll() {
    let mut fixture = Fixture::new(false);
    fixture.window.long_press("drag-1").unwrap();
    assert!(!fixture.window.kernel().exists("row-1"), "the spent pointer does not retain the row after delivery");
    assert!(fixture.window.held_pointer().is_none());
    fixture.clean();
}

#[test]
fn resetting_bounded_collection_history_preserves_surviving_pins_and_retires_absent_keys() {
    let app = App::new(HeadlessPlatform::default());
    let items = app.run(|| state_list("items", || (0..ROWS).map(|key| (key, format!("Item {key}"))).collect(), |item| item.0));
    let drags = Rc::new(RefCell::new(Vec::new()));
    let seen = drags.clone();
    let mut window = app
        .open_window(WindowOptions::new("Collection history reset").size(640.0, 192.0), move || {
            let seen = seen.clone();
            col().size_full().id("list").overflow_y_scroll().child(virtual_each(
                move || items.read_only(),
                |_| ROW_HEIGHT,
                move |key, item| {
                    let draft = state("draft", || "seed".to_owned());
                    let seen = seen.clone();
                    row()
                        .id(format!("row-{key}"))
                        .h(ROW_HEIGHT)
                        .flex_shrink_0()
                        .child(text_input().id(format!("editor-{key}")).w(240).value(draft).on_input(move |value| draft.set(value)))
                        .child(
                            el().id(format!("drag-{key}"))
                                .w(120)
                                .h(40)
                                .preserve_focus_on_press(true)
                                .child(text(move || item.with(|item| item.1.clone())).id(format!("label-{key}")))
                                .on_drag(move |drag| seen.borrow_mut().push(drag.phase)),
                        )
                },
            ))
        })
        .unwrap();
    window.dispatch(Event::Focus("editor-0".into())).unwrap();
    window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 4)) });
    let editor = target(&window, "editor-0");
    let drag = target(&window, "drag-1");
    let (x, y) = center(&window, "drag-1");
    window.pointer_down(x, y);
    window.pointer_move(x + 12.0, y);
    window.dispatch(Event::ScrollTo("list".into(), 0.0, FAR)).unwrap();
    for remove_pins in [false, true] {
        app.run(|| {
            if remove_pins {
                items.remove(&0);
                items.remove(&1);
            }
            // More than the 4,096-entry history budget forces an authoritative reset.
            for key in ROWS..ROWS + 5_000 {
                items.push((key, "temporary".into()));
            }
            for key in ROWS..ROWS + 5_000 {
                items.remove(&key);
            }
            items.update(&1, |item| item.1 = "Latest row input".into());
        });
        app.settle();
        if remove_pins {
            assert!(!window.kernel().exists("row-0"));
            assert!(!window.kernel().exists("row-1"));
            assert!(window.text_field().is_none());
            assert!(window.held_pointer().is_none());
            stale_type(&mut window, "editor-0", &editor);
        } else {
            assert_eq!(target(&window, "editor-0"), editor);
            assert_eq!(target(&window, "drag-1"), drag);
            assert_eq!(window.text_field().unwrap().composition, "nihao");
            assert_eq!(window.kernel().text_of("label-1").as_deref(), Some("Latest row input"));
            window.pointer_move(x + 24.0, y);
            assert_eq!(drags.borrow().last(), Some(&DragPhase::Move));
        }
    }
    assert!(row_ids(&window).len() < 24);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn a_raw_pointer_up_callback_can_close_its_window_or_shut_down_its_app() {
    for shutdown in [false, true] {
        let mut window = Window::headless(move || {
            let id = current_window();
            col().size_full().overflow_y_scroll().child(virtual_each(
                || (0..100u32).map(|key| (key, key)).collect::<Vec<_>>(),
                |_| ROW_HEIGHT,
                move |key, _| {
                    el().id(format!("close-{key}")).h(ROW_HEIGHT).flex_shrink_0().on_press(move || {
                        if shutdown {
                            App::current().shutdown();
                        } else {
                            close_window(id);
                        }
                    })
                },
            ))
        });
        let (x, y) = center(&window, "close-0");
        window.pointer_down(x, y);
        window.pointer_up(x, y);
        assert!(window.is_closed());
        assert!(window.held_pointer().is_none());
        assert_eq!(window.app().is_shutdown(), shutdown);
        assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
        window.app().shutdown();
    }
}

#[test]
fn row_owned_overlay_editors_pin_their_owner_until_close_or_data_removal() {
    for window_overlay in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let (items, opened) = app.run(|| (state_list("items", || (0..ROWS).collect(), |key| *key), state("opened", || None)));
        let mut window = app
            .open_window(WindowOptions::new("Row-owned overlay").size(640.0, 192.0), move || {
                col().size_full().id("list").overflow_y_scroll().child(virtual_each(
                    move || items.read_only(),
                    |_| ROW_HEIGHT,
                    move |key, _| {
                        let key = *key;
                        let content = move || {
                            let draft = state("draft", || "seed".to_owned());
                            col()
                                .id("row-overlay")
                                .w(300)
                                .h(80)
                                .modal(false, move || opened.set(None))
                                .child(text_input().id("overlay-editor").w(240).value(draft).on_input(move |value| draft.set(value)))
                        };
                        let row = el().id(format!("row-{key}")).h(ROW_HEIGHT).flex_shrink_0().child(format!("Row {key}"));
                        if window_overlay {
                            row.overlay_window(move || opened.get() == Some(key), content)
                        } else {
                            row.overlay(move || opened.get() == Some(key), Placement::BELOW, content)
                        }
                    },
                ))
            })
            .unwrap();
        for remove_data in [false, true] {
            window.dispatch(Event::ScrollTo("list".into(), 0.0, 0.0)).unwrap();
            app.run(|| opened.set(Some(0)));
            app.settle();
            window.dispatch(Event::Focus("overlay-editor".into())).unwrap();
            window.set_selection(4, 1);
            window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 4)) });
            let editor = target(&window, "overlay-editor");
            let owner = target(&window, "row-0");
            window.dispatch(Event::ScrollTo("list".into(), 0.0, FAR)).unwrap();
            assert_eq!(target(&window, "row-0"), owner);
            assert_eq!(target(&window, "overlay-editor"), editor);
            assert_eq!(window.inspect().focus.as_deref(), Some("overlay-editor"));
            let field = window.text_field().unwrap();
            assert_eq!(field.selection, 1..4);
            assert!(field.reversed);
            assert_eq!(field.composition, "nihao");
            assert_eq!(field.composition_selection, Some(1..4));
            assert!(row_ids(&window).len() < 24);
            assert!(!window.kernel().exists("row-100"));
            app.run(|| {
                if remove_data {
                    items.remove(&0);
                } else {
                    opened.set(None);
                }
            });
            app.settle();
            assert!(!window.kernel().exists("row-overlay"));
            assert!(!window.kernel().exists("row-0"));
            assert!(window.text_field().is_none());
            assert!(window.kernel().children(window.kernel().overlay_layer()).is_empty());
            stale_type(&mut window, "overlay-editor", &editor);
        }
        assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}
