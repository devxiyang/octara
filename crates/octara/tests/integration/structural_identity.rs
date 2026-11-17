//! Typed keys identify owners and selection paths; labels never merge them.

use octara::inspect::{self, Delivery, Request};
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key(u32);

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        0u8.hash(state); // Deliberate collisions must still resolve through typed equality.
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("same/row:%2F]/scope[same")
    }
}

#[ui]
fn counter(key: Key) -> El {
    let value = state("local", || key.0);
    el().id(format!("row-{}", key.0))
        .role(Role::Button)
        .label(format!("Row {}", key.0))
        .h(40)
        .flex_shrink_0()
        .child(text(move || value.get().to_string()).id(format!("value-{}", key.0)))
        .on_press(move || value.update(|value| *value += 10))
}

fn value(window: &Window, key: u32) -> String {
    window.kernel().text_of(&format!("value-{key}")).unwrap()
}

fn target(window: &Window, key: u32) -> String {
    window.inspect().find(&format!("row-{key}")).unwrap().target.clone()
}

fn clean(window: &Window) {
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
}

#[test]
fn same_label_and_hash_rows_keep_independent_state_through_reorder_reload_and_removal() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let items = app.run(|| state("items", || vec![(Key(1), ()), (Key(2), ())]));
        let mut window = app
            .open_window(WindowOptions::default(), move || {
                let row = |key: &Key, _: ReadSignal<()>| counter(*key).into();
                col()
                    .size_full()
                    .overflow_y_scroll()
                    .child(if virtualized { virtual_each(move || items.get(), |_| 40.0, row) } else { each(move || items.get(), row) })
                    .into()
            })
            .unwrap();
        assert_eq!((value(&window, 1), value(&window, 2)), ("1".into(), "2".into()));
        window.tap("row-2").unwrap();
        let before = target(&window, 2);
        app.run(|| items.set(vec![(Key(2), ()), (Key(1), ())]));
        app.settle();
        assert_eq!(target(&window, 2), before);
        assert_eq!((value(&window, 1), value(&window, 2)), ("1".into(), "12".into()));
        app.hot_reload();
        assert_ne!(target(&window, 2), before);
        assert_eq!((value(&window, 1), value(&window, 2)), ("1".into(), "12".into()));
        let stale = inspect::handle_line(&mut window, &octara::serde_json::json!({"op":"tap", "id":"row-2", "target":before}).to_string());
        assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
        assert_eq!(stale.delivery, Some(Delivery::Refused));
        app.run(|| items.set(vec![(Key(1), ())]));
        app.settle();
        app.run(|| items.set(vec![(Key(1), ()), (Key(2), ())]));
        app.settle();
        assert_eq!((value(&window, 1), value(&window, 2)), ("1".into(), "2".into()));
        clean(&window);
        app.shutdown();
    }
}

#[test]
fn explicit_scope_uses_key_type_and_equality_across_reordered_reload() {
    let reverse = Rc::new(Cell::new(false));
    let order = reverse.clone();
    let mut window = Window::headless(move || {
        let keys = if order.get() { [Key(2), Key(1)] } else { [Key(1), Key(2)] };
        col().size_full().children(keys.map(|key| scope(key, move || counter(key)))).into()
    });
    window.tap("row-1").unwrap();
    reverse.set(true);
    window.app().hot_reload();
    assert_eq!((value(&window, 1), value(&window, 2)), ("11".into(), "2".into()));
    clean(&window);

    let window = Window::headless(|| {
        col()
            .size_full()
            .child(scope(1u32, || counter(Key(3))))
            .child(scope(1u64, || counter(Key(4))))
            .child(scope("1", || counter(Key(5))))
            .into()
    });
    assert_eq!((value(&window, 3), value(&window, 4), value(&window, 5)), ("3".into(), "4".into(), "5".into()));
    clean(&window);
}

#[test]
fn keyed_branches_reset_on_change_and_only_recover_the_equal_key_on_reload() {
    let selected = Rc::new(Cell::new(Key(1)));
    let source = selected.clone();
    let mut window = Window::headless(move || {
        keyed(
            {
                let source = source.clone();
                move || source.get()
            },
            |key| counter(*key).into(),
        )
    });
    window.tap("row-1").unwrap();
    selected.set(Key(2));
    window.app().hot_reload();
    assert_eq!(value(&window, 2), "2", "a new same-label key cannot reclaim the old branch's state");
    window.tap("row-2").unwrap();
    window.app().hot_reload();
    assert_eq!(value(&window, 2), "12");
    clean(&window);

    let app = App::new(HeadlessPlatform::default());
    let selected = app.run(|| state("selected", || Key(1)));
    let mut window = app.open_window(WindowOptions::default(), move || keyed(move || selected.get(), |key| counter(*key).into())).unwrap();
    window.tap("row-1").unwrap();
    app.run(|| selected.set(Key(2)));
    app.settle();
    app.run(|| selected.set(Key(1)));
    app.settle();
    assert_eq!(value(&window, 1), "1", "retiring a branch retires its local state");
    clean(&window);
    app.shutdown();
}

#[test]
fn virtual_selection_reads_each_same_label_key_and_retiring_rows_reset_their_state() {
    let mut window = Window::headless(|| {
        col()
            .id("list")
            .size_full()
            .selectable()
            .overflow_y_scroll()
            .child(virtual_each(|| (0..80).map(|id| (Key(id), ())).collect::<Vec<_>>(), |_| 40.0, |key, _| counter(*key).into()))
            .into()
    });
    window.tap("row-1").unwrap();
    let before = target(&window, 1);
    assert!(inspect::handle(&mut window, Request::Select { from: None, to: None, region: Some("list".into()) }).ok);
    let expected = (0..80).map(|id| if id == 1 { "11".into() } else { id.to_string() }).collect::<Vec<_>>().join("\n\n");
    assert_eq!(window.selected_text().as_deref(), Some(expected.as_str()));
    assert_eq!(value(&window, 1), "11", "offscreen copy owners do not replace mounted state");
    window.dispatch(Event::ScrollTo("list".into(), 0.0, 2_500.0)).unwrap();
    assert!(!window.kernel().exists("row-1"));
    window.dispatch(Event::ScrollTo("list".into(), 0.0, 0.0)).unwrap();
    assert_ne!(target(&window, 1), before);
    assert_eq!(value(&window, 1), "1");
    clean(&window);
}

#[test]
fn colliding_incremental_keys_keep_their_owner_through_insert_move_and_remove() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let (items, sorted) = app.run(|| {
            let items = state_list("items", || vec![(Key(1), 1u32), (Key(2), 2)], |item| item.0);
            (items, items.sort_by(|item| item.1))
        });
        let mut window = app
            .open_window(WindowOptions::default(), move || {
                let sorted = sorted.clone();
                let row = |key: &Key, _: ReadSignal<(Key, u32)>| counter(*key).into();
                col()
                    .id("list")
                    .size_full()
                    .overflow_y_scroll()
                    .child(if virtualized {
                        virtual_each(move || sorted.clone(), |_| 40.0, row)
                    } else {
                        each(move || sorted.clone(), row)
                    })
                    .into()
            })
            .unwrap();
        window.tap("row-2").unwrap();
        let before = target(&window, 2);
        app.run(|| {
            items.insert(0, (Key(3), 3));
            items.update(&Key(2), |item| item.1 = 0);
        });
        app.settle();
        assert_eq!(target(&window, 2), before);
        assert_eq!((value(&window, 1), value(&window, 2), value(&window, 3)), ("1".into(), "12".into(), "3".into()));
        app.run(|| items.remove(&Key(2)));
        app.settle();
        app.run(|| items.insert(0, (Key(2), 0)));
        app.settle();
        assert_ne!(target(&window, 2), before);
        assert_eq!(value(&window, 2), "2");
        clean(&window);
        app.shutdown();
    }
}

struct SampledKey {
    noise: ReadSignal<u32>,
    armed: Rc<Cell<bool>>,
}

impl SampledKey {
    fn sample(&self) {
        if self.armed.get() {
            self.noise.get();
        }
    }
}

impl PartialEq for SampledKey {
    fn eq(&self, _: &Self) -> bool {
        self.sample();
        true
    }
}

impl Eq for SampledKey {}

impl Hash for SampledKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.sample();
        0u8.hash(state);
    }
}

impl fmt::Display for SampledKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.sample();
        f.write_str("same")
    }
}

impl Drop for SampledKey {
    fn drop(&mut self) {
        self.sample();
    }
}

#[test]
fn key_callbacks_and_equal_temporary_key_retirement_do_not_subscribe_the_branch() {
    let app = App::new(HeadlessPlatform::default());
    let (revision, noise) = app.run(|| (state("revision", || 0), state("noise", || 0)));
    let runs = Rc::new(Cell::new(0));
    let observed = runs.clone();
    let armed = Rc::new(Cell::new(true));
    let callbacks = armed.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            let (observed, callbacks) = (observed.clone(), callbacks.clone());
            keyed(
                move || {
                    revision.get();
                    observed.set(observed.get() + 1);
                    SampledKey { noise: noise.read_only(), armed: callbacks.clone() }
                },
                |_| text("Retained").into(),
            )
        })
        .unwrap();
    assert_eq!(runs.get(), 1);
    app.run(|| revision.set(1));
    app.settle();
    assert_eq!(runs.get(), 2, "the equal new key runs Eq and retires its temporary value");
    app.run(|| noise.set(7));
    app.settle();
    assert_eq!(runs.get(), 2, "Hash, Display, Eq and Drop reads must all remain sampled");
    clean(&window);
    app.close_window(window.id()); // Pruning the canonical value also samples while App state is alive.
    armed.set(false);
    app.shutdown();
}

#[ui]
fn named_row(label: &'static str) -> View {
    each(
        || vec![(Key(1), ())],
        move |_, _| {
            let value = state("local", || label);
            text(value).id(label).into()
        },
    )
}

#[test]
fn selection_row_keys_include_the_stable_owner_path_and_list_region() {
    for separate_owners in [false, true] {
        let mut window = Window::headless(move || {
            col()
                .id("list")
                .size_full()
                .selectable()
                .child(if separate_owners { scope("left-owner", || named_row("Left")) } else { named_row("Left") })
                .child(if separate_owners { scope("right-owner", || named_row("Right")) } else { named_row("Right") })
                .into()
        });
        let path = {
            let kernel = window.kernel();
            let region = kernel.find("list").unwrap();
            let right = kernel.find("Right").unwrap();
            let path = kernel.path_to(region, right).unwrap();
            assert_eq!(kernel.resolve_path(region, &path), Some(right), "equal keys in different regions identify different rows");
            path
        };
        window.select(("Right", 0), ("Right", 5)).unwrap();
        assert_eq!(window.selected_text().as_deref(), Some("Right"));
        window.select(("Left", 0), ("Left", 4)).unwrap();
        assert_eq!(window.selected_text().as_deref(), Some("Left"));
        window.select(("Left", 0), ("Right", 5)).unwrap();
        assert_eq!(window.selected_text().as_deref(), Some("Left\n\nRight"));
        window.app().hot_reload();
        let kernel = window.kernel();
        assert_eq!(kernel.resolve_path(kernel.find("list").unwrap(), &path), kernel.find("Right"), "row namespaces survive reload");
        drop(kernel);
        clean(&window);
    }
}
