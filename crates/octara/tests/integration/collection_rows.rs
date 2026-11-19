//! One row identity and current-input contract for snapshots and incremental sources.
use octara::app::{Event, InputEvent};
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use octara::reactive::untrack;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn target(window: &Window, id: &str) -> String {
    window.inspect().find(id).unwrap().target.clone()
}

#[test]
fn typed_editor_rows_keep_their_edit_session_when_files_reorder() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let files = app.run(|| state("files", || vec![(1u32, "A😀Z".to_owned()), (2, "A😀Z".to_owned())]));
        let submissions = app.run(|| state("submissions", Vec::<(u32, String)>::new));
        let mut window = app
            .open_window(WindowOptions::new("File names"), move || {
                let row = move |key: &u32, name: ReadSignal<String>| {
                    let id = *key;
                    let draft = state("draft", || name.get());
                    input()
                        .id(format!("name-{id}"))
                        .label("File name")
                        .value(draft)
                        .h(32)
                        .on_input(move |text| draft.set(text))
                        .on_submit(move || submissions.update(|names| names.push((id, draft.get()))))
                };
                let rows = if virtualized { virtual_each(move || files.get(), |_| 32.0, row) } else { each(move || files.get(), row) };
                col().size_full().overflow_y_scroll().child(rows)
            })
            .unwrap();
        let identity = target(&window, "name-1");
        window.dispatch(Event::Focus("name-1".into())).unwrap();
        window.set_selection(1, 5);
        window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((5, 5)) });

        app.run(|| files.update(|files| files.reverse()));
        let snapshot = window.inspect();
        assert_eq!(snapshot.focus.as_deref(), Some("name-1"));
        assert_eq!(snapshot.find("name-1").unwrap().target, identity);
        let edit = snapshot.text_input.unwrap();
        assert_eq!((edit.text.as_str(), edit.composition.as_str(), edit.selection), ("A😀Z", "nihao", [1, 5]));

        window.press_key("enter").unwrap();
        assert!(app.run(|| submissions.with(Vec::is_empty)));
        window.event(InputEvent::Text("你好".into()));
        assert_eq!(window.inspect().text_input.unwrap().text, "A你好Z");
        window.press_key("enter").unwrap();
        assert_eq!(app.run(|| submissions.get()), [(1, "A你好Z".to_owned())]);
        window.press_key("mod+z").unwrap();
        assert_eq!(window.inspect().text_input.unwrap().text, "A😀Z");
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn retiring_from_row_cleanup_skips_replacement_rows() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let items = app.run(|| state_list("items", || vec![1u32], |item| *item));
        let baseline = app.work().reactive_nodes;
        let events = Rc::new(RefCell::new(Vec::new()));
        let mounted = events.clone();
        let list = app.run(|| items.read_only());
        let window = app
            .open_window(WindowOptions::default(), move || {
                let (list, mounted) = (list.clone(), mounted.clone());
                let source = move || list.clone();
                let row = move |key: &u32, item: ReadSignal<u32>| {
                    mounted.borrow_mut().push(*key);
                    if *key == 1 {
                        let cleaned = mounted.clone();
                        on_cleanup(move || {
                            cleaned.borrow_mut().push(0);
                            close_window(current_window());
                        });
                    }
                    text(move || item.get().to_string()).id(format!("row-{key}")).h(44).flex_shrink_0()
                };
                col().size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(source, |_| 44.0, row)
                } else {
                    each(source, row)
                })
            })
            .unwrap();

        assert!(window.kernel().exists("row-1"));
        app.run(|| {
            items.remove(&1);
            items.push(2);
        });
        assert_eq!(*events.borrow(), [1, 0], "virtualized={virtualized}");
        assert!(window.is_closed());
        assert!(!app.is_failed());
        assert_eq!(app.work().reactive_nodes, baseline);
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn retiring_during_row_mount_skips_following_row_factories() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let built = Rc::new(RefCell::new(Vec::new()));
        let mounted = built.clone();
        let window = app
            .open_window(WindowOptions::default(), move || {
                let mounted = mounted.clone();
                let source = || vec![(1u32, 1u32), (2, 2)];
                let row = move |key: &u32, item: ReadSignal<u32>| {
                    mounted.borrow_mut().push(*key);
                    if *key == 1 {
                        watch(|| (), |_, _| close_window(current_window()));
                    }
                    text(move || item.get().to_string()).h(44).flex_shrink_0()
                };
                col().size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(source, |_| 44.0, row)
                } else {
                    each(source, row)
                })
            })
            .unwrap();

        assert_eq!(*built.borrow(), [1], "virtualized={virtualized}");
        assert!(window.is_closed());
        assert!(!app.is_failed());
        assert_eq!(app.work().reactive_nodes, 0);
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn snapshots_preserve_rows_but_replace_inputs_and_retire_removed_keys() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let items = app.run(|| state("items", || vec![(1u32, "one".to_owned()), (2, "two".to_owned())]));
        let builds = Rc::new(Cell::new(0));
        let built = builds.clone();
        let mut window = app
            .open_window(WindowOptions::default(), move || {
                let built = built.clone();
                let row = move |key: &u32, item: ReadSignal<String>| {
                    built.set(built.get() + 1);
                    let count = state("count", || 0);
                    col()
                        .id(format!("row-{key}"))
                        .h(60)
                        .flex_shrink_0()
                        .child(text(item).id(format!("value-{key}")))
                        .child(text(move || count.get().to_string()).id(format!("count-{key}")))
                        .child(el().id(format!("inc-{key}")).size_8().on_press(move || count.update(|n| *n += 1)))
                };
                col().size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(move || items.get(), |_| 60.0, row)
                } else {
                    each(move || items.get(), row)
                })
            })
            .unwrap();
        window.tap("inc-1").unwrap();
        let first = target(&window, "row-1");
        app.run(|| items.set(vec![(2, "TWO".into()), (1, "ONE".into())]));
        app.settle();
        assert_eq!(target(&window, "row-1"), first);
        assert_eq!(window.kernel().text_of("value-1").as_deref(), Some("ONE"));
        assert_eq!(window.kernel().text_of("count-1").as_deref(), Some("1"));
        assert_eq!(builds.get(), 2);
        app.run(|| items.set(vec![(2, "TWO".into())]));
        app.settle();
        assert!(!window.kernel().exists("row-1"));
        app.run(|| items.set(vec![(1, "again".into())]));
        app.settle();
        assert_ne!(target(&window, "row-1"), first);
        assert_eq!(window.kernel().text_of("count-1").as_deref(), Some("0"));
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn retained_row_reads_current_source_inside_the_switching_handler() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let (a, b, choose_b) = app.run(|| {
            (
                state_list("a", || vec![(1u32, "old".to_owned())], |v| v.0),
                state_list("b", || vec![(1u32, "new".to_owned())], |v| v.0),
                state("choose_b", || false),
            )
        });
        let seen = Rc::new(RefCell::new(String::new()));
        let saved = seen.clone();
        let mut window = app
            .open_window(WindowOptions::default(), move || {
                let saved = saved.clone();
                let source = move || if choose_b.get() { b.read_only() } else { a.read_only() };
                let row = move |_: &u32, item: ReadSignal<(u32, String)>| {
                    let saved = saved.clone();
                    col().id("row").h(80).flex_shrink_0().child(text(move || item.with(|v| v.1.clone())).id("value")).child(
                        el().id("switch").size_8().on_press(move || {
                            choose_b.set(true);
                            *saved.borrow_mut() = item.with(|v| v.1.clone());
                        }),
                    )
                };
                col().size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(source, |_| 80.0, row)
                } else {
                    each(source, row)
                })
            })
            .unwrap();
        let before = target(&window, "row");
        window.tap("switch").unwrap();
        assert_eq!(&*seen.borrow(), "new");
        assert_eq!(target(&window, "row"), before);
        assert_eq!(window.kernel().text_of("value").as_deref(), Some("new"));
        app.run(|| a.update(&1, |v| v.1 = "stale".into()));
        app.settle();
        assert_eq!(window.kernel().text_of("value").as_deref(), Some("new"));
        app.run(|| b.update(&1, |v| v.1 = "latest".into()));
        app.settle();
        assert_eq!(window.kernel().text_of("value").as_deref(), Some("latest"));
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn invalid_snapshot_retires_targets_and_recovers_without_reusing_them() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let items = app.run(|| state("items", || vec![(1u32, "valid")]));
        let window = app
            .open_window(WindowOptions::default(), move || {
                let row = |_: &u32, item: ReadSignal<&'static str>| text(item).id("row").h(32).flex_shrink_0();
                col().size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(move || items.get(), |_| 32.0, row)
                } else {
                    each(move || items.get(), row)
                })
            })
            .unwrap();
        let before = target(&window, "row");
        app.run(|| items.set(vec![(1, "duplicate"), (1, "invalid")]));
        // Diagnostics remain visible, but propagation itself recovers normally.
        app.settle();
        assert!(!window.kernel().exists("row"));
        assert!(app.diagnostics().iter().any(|d| d.code == "DUPLICATE_KEY"));
        app.run(|| items.set(vec![(1, "recovered")]));
        app.settle();
        assert_ne!(target(&window, "row"), before);
        assert_eq!(window.kernel().text_of("row").as_deref(), Some("recovered"));
        assert!(!app.diagnostics().iter().any(|d| d.code == "LIST_ITEM_UNAVAILABLE"));
        app.shutdown();
    }
}

#[test]
fn transient_incremental_keys_are_not_built_and_reinsertion_retires_identity() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let items = app.run(|| state_list("items", || vec![(1u32, "old")], |v| v.0));
        let window = app
            .open_window(WindowOptions::default(), move || {
                let row = |key: &u32, item: ReadSignal<(u32, &'static str)>| {
                    text(move || item.with(|v| v.1)).id(format!("row-{key}")).h(32).flex_shrink_0()
                };
                col().size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(move || items.read_only(), |_| 32.0, row)
                } else {
                    each(move || items.read_only(), row)
                })
            })
            .unwrap();
        let before = target(&window, "row-1");
        app.run(|| {
            items.push((2, "transient"));
            items.remove(&2);
            items.remove(&1);
            items.push((1, "new"));
        });
        app.settle();
        assert!(!window.kernel().exists("row-2"));
        assert_ne!(target(&window, "row-1"), before);
        assert_eq!(window.kernel().text_of("row-1").as_deref(), Some("new"));
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn snapshot_reads_pull_new_values_without_flushing_and_fail_after_removal() {
    let app = App::new(HeadlessPlatform::default());
    let items = app.run(|| state("items", || vec![(1u32, "old".to_owned())]));
    let seen = Rc::new(RefCell::new(String::new()));
    let saved = seen.clone();
    let mut window = app
        .open_window(WindowOptions::default(), move || {
            let saved = saved.clone();
            col().size_full().child(each(
                move || items.get(),
                move |_, item| {
                    let saved = saved.clone();
                    el().id("row").size_10().child(text(item)).on_press(move || {
                        items.set(vec![(1, "new".into())]);
                        *saved.borrow_mut() = item.get();
                        items.set(vec![]);
                        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| item.get())).is_err());
                    })
                },
            ))
        })
        .unwrap();
    window.tap("row").unwrap();
    assert_eq!(&*seen.borrow(), "new");
    assert!(!window.kernel().exists("row"));
    assert!(app.diagnostics().iter().any(|d| d.code == "LIST_ITEM_UNAVAILABLE"));
    app.shutdown();
}

#[test]
fn structural_work_queued_by_an_effect_preempts_a_retiring_rows_effect() {
    let app = App::new(HeadlessPlatform::default());
    let trigger = app.run(|| state("trigger", || false));
    let retired_effects = Rc::new(Cell::new(0));
    let calls = retired_effects.clone();
    let _window = app
        .open_window(WindowOptions::default(), move || {
            let items = state("items", || vec![(1u32, "old")]);
            watch(
                move || trigger.get(),
                move |trigger, _| {
                    if *trigger {
                        items.set(vec![]);
                    }
                },
            );
            let calls = calls.clone();
            col().size_full().child(each(
                move || items.get(),
                move |_, item| {
                    let calls = calls.clone();
                    watch(
                        move || trigger.get(),
                        move |trigger, _| {
                            if *trigger {
                                calls.set(calls.get() + 1);
                                // This is invalid once the earlier effect removes our key.
                                item.get();
                            }
                        },
                    );
                    text(item)
                },
            ))
        })
        .unwrap();
    app.run(|| trigger.set(true));
    assert_eq!(retired_effects.get(), 0);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn streamed_buffer_edits_do_not_recompute_the_snapshot_or_rebuild_rows() {
    let app = App::new(HeadlessPlatform::default());
    let buffer = app.run(|| TextBuffer::new("start"));
    let items = app.run(|| state("items", || vec![(1u32, buffer.clone())]));
    let snapshots = Rc::new(Cell::new(0));
    let builds = Rc::new(Cell::new(0));
    let (computed, built) = (snapshots.clone(), builds.clone());
    let window = app
        .open_window(WindowOptions::default(), move || {
            let (computed, built) = (computed.clone(), built.clone());
            col().size_full().child(each(
                move || {
                    computed.set(computed.get() + 1);
                    items.get()
                },
                move |_, item| {
                    built.set(built.get() + 1);
                    text(untrack(|| item.get())).id("stream")
                },
            ))
        })
        .unwrap();
    for _ in 0..100 {
        app.run(|| buffer.append("!"));
    }
    assert_eq!(snapshots.get(), 1);
    assert_eq!(builds.get(), 1);
    assert_eq!(window.kernel().text_of("stream").unwrap().len(), 105);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn overflowing_incremental_history_reconciles_final_membership_and_preserves_survivors() {
    for virtualized in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let items = app.run(|| state_list("items", || vec![(1u32, "old"), (2, "kept")], |item| item.0));
        let source = app.run(|| items.read_only().filter(|_| true));
        let builds = Rc::new(Cell::new(0));
        let built = builds.clone();
        let window = app
            .open_window(WindowOptions::default(), move || {
                let source = source.clone();
                let built = built.clone();
                let row = move |key: &u32, item: ReadSignal<(u32, &'static str)>| {
                    built.set(built.get() + 1);
                    text(move || item.with(|item| item.1)).id(format!("row-{key}")).h(32).flex_shrink_0()
                };
                col().id("rows").size_full().overflow_y_scroll().child(if virtualized {
                    virtual_each(move || source.clone(), |_| 32.0, row)
                } else {
                    each(move || source.clone(), row)
                })
            })
            .unwrap();
        let before = target(&window, "row-1");
        let kept = target(&window, "row-2");
        app.run(|| {
            // The removed-and-reinserted key is older than the retained history tail.
            items.remove(&1);
            items.push((1, "replacement"));
            for _ in 0..5000 {
                items.push((99, "transient"));
                items.remove(&99);
            }
            items.push((3, "last"));
        });
        assert_eq!(target(&window, "row-1"), before, "unobserved absence is not a published removal");
        assert_eq!(target(&window, "row-2"), kept);
        assert_eq!(window.kernel().text_of("row-1").as_deref(), Some("replacement"));
        assert_eq!(window.kernel().text_of("row-2").as_deref(), Some("kept"));
        assert_eq!(window.kernel().text_of("row-3").as_deref(), Some("last"));
        assert!(!window.kernel().exists("row-99"));
        {
            let kernel = window.kernel();
            let rows = kernel.find("rows").unwrap();
            let order: Vec<_> =
                kernel.children(rows).iter().filter_map(|node| kernel.test_id(*node)).filter(|id| id.starts_with("row-")).collect();
            assert_eq!(order, ["row-2", "row-1", "row-3"]);
        }
        assert_eq!(builds.get(), 3, "only the added final row is constructed");
        let recovered = target(&window, "row-1");
        app.run(|| items.update(&1, |item| item.1 = "incremental again"));
        assert_eq!(target(&window, "row-1"), recovered);
        assert_eq!(window.kernel().text_of("row-1").as_deref(), Some("incremental again"));
        assert_eq!(builds.get(), 3);
        app.run(|| {
            items.remove(&2);
            for _ in 0..5000 {
                items.push((99, "transient"));
                items.remove(&99);
            }
        });
        assert!(!window.kernel().exists("row-2"), "absence in the recovered snapshot retires the row");
        app.run(|| items.push((2, "back")));
        assert_ne!(target(&window, "row-2"), kept, "published absence requires a fresh target");
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}
