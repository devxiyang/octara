//! Ordinary functions compose UI; explicitly keyed scopes own state and lifetime.

use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn counter(id: &'static str, value: Signal<u32>) -> El {
    el().id(id)
        .role(Role::Button)
        .label(id)
        .size_12()
        .child(text(move || value.get().to_string()).id(format!("{id}-value")))
        .on_press(move || value.update(|value| *value += 1))
}

#[ui]
fn owned_counter(key: &'static str, id: &'static str, initial: u32) -> View {
    scope(key, move || {
        let value = state("value", || initial);
        counter(id, value)
    })
}

fn clean(window: &Window) {
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
}

#[test]
fn ordinary_functions_reuse_caller_owned_state_without_an_annotation_or_scope() {
    let mut window = Window::headless(|| {
        let left = state("left", || 0u32);
        let right = state("right", || 10u32);
        col().size_full().child(counter("left", left)).child(counter("right", right))
    });
    let left = window.inspect().find("left").unwrap().target.clone();
    window.tap("left").unwrap();
    window.tap("right").unwrap();
    window.tap("right").unwrap();
    assert_eq!(window.kernel().text_of("left-value").as_deref(), Some("1"));
    assert_eq!(window.kernel().text_of("right-value").as_deref(), Some("12"));
    assert_eq!(window.inspect().find("left").unwrap().target, left);
    assert_eq!(window.inspect().find("left").unwrap().component, None);
    clean(&window);
}

#[test]
fn scope_accepts_elements_and_borrowed_or_owned_strings_without_a_wrapper_node() {
    let window = Window::headless(|| {
        col()
            .id("content")
            .size_full()
            .child(scope(7, || text("Element").id("element")))
            .child(scope("borrowed", || "Borrowed"))
            .child(scope(String::from("owned"), || String::from("Owned")))
    });
    let snapshot = window.inspect();
    let content = snapshot.find("content").unwrap();
    let names: Vec<_> = content.children.iter().map(|node| node.name.as_deref()).collect();
    assert_eq!(names, [Some("Element"), Some("Borrowed"), Some("Owned")]);
    assert_eq!(content.children[0].id.as_deref(), Some("element"));
    clean(&window);
}

#[test]
fn show_builds_and_releases_elements_with_the_visible_branch() {
    let builds = Rc::new(Cell::new(0));
    let cleanups = Rc::new(Cell::new(0));
    let (observed_builds, observed_cleanups) = (builds.clone(), cleanups.clone());
    let mut window = Window::headless(move || {
        let visible = state("visible", || false);
        on_command("toggle", move || visible.update(|on| *on = !*on));
        let (builds, cleanups) = (observed_builds.clone(), observed_cleanups.clone());
        show(
            move || visible.get(),
            move || {
                builds.set(builds.get() + 1);
                let cleanups = cleanups.clone();
                on_cleanup(move || cleanups.set(cleanups.get() + 1));
                counter("details", state("value", || 0))
            },
        )
    });

    assert_eq!(builds.get(), 0);
    assert!(!window.kernel().exists("details"));
    window.invoke("toggle").unwrap();
    let first_target = window.inspect().find("details").unwrap().target.clone();
    window.tap("details").unwrap();
    assert_eq!(window.kernel().text_of("details-value").as_deref(), Some("1"));
    assert_eq!(builds.get(), 1);
    assert_eq!(window.inspect().find("details").unwrap().target, first_target);

    window.invoke("toggle").unwrap();
    assert!(!window.kernel().exists("details"));
    assert_eq!(cleanups.get(), 1);
    window.invoke("toggle").unwrap();
    assert_eq!(builds.get(), 2);
    assert_eq!(window.kernel().text_of("details-value").as_deref(), Some("0"));
    assert_ne!(window.inspect().find("details").unwrap().target, first_target);
    clean(&window);
    window.app().shutdown();
    assert_eq!(cleanups.get(), 2);
}

#[test]
fn duplicate_scope_keys_skip_the_body_and_leave_later_siblings_usable() {
    let bodies = Rc::new(Cell::new(0));
    let cleaned = Rc::new(RefCell::new(Vec::new()));
    let (out, cleanup) = (bodies.clone(), cleaned.clone());
    let mut window = Window::headless(move || {
        let (first, duplicate) = (out.clone(), out.clone());
        let (first_cleanup, duplicate_cleanup) = (cleanup.clone(), cleanup.clone());
        col()
            .size_full()
            .child(scope("repeated", move || {
                first.set(first.get() + 1);
                let value = state("value", || 10u32);
                on_cleanup(move || first_cleanup.borrow_mut().push("first"));
                counter("first", value)
            }))
            .child(scope("repeated", move || {
                duplicate.set(duplicate.get() + 1);
                let value = state("value", || 99u32);
                on_cleanup(move || duplicate_cleanup.borrow_mut().push("duplicate"));
                counter("duplicate", value)
            }))
            .child(owned_counter("later", "later", 0))
    });
    assert_eq!(bodies.get(), 1, "duplicate content never runs");
    assert!(!window.kernel().exists("duplicate"));
    window.tap("first").unwrap();
    window.tap("later").unwrap();
    assert_eq!(window.kernel().text_of("first-value").as_deref(), Some("11"));
    assert_eq!(window.kernel().text_of("later-value").as_deref(), Some("1"));
    let diagnostics = window.app().diagnostics();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.id, "DUPLICATE_SCOPE_KEY");
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    window.app().shutdown();
    assert_eq!(&*cleaned.borrow(), &["first"], "the refused scope did not register cleanup");
}

#[test]
fn scoped_providers_handlers_and_cleanup_keep_their_owner() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let cleaned = Rc::new(RefCell::new(Vec::new()));
    let (out, cleanup) = (seen.clone(), cleaned.clone());
    let mut window = Window::headless(move || {
        provide(1u32);
        let visible = state("visible", || true);
        let (root, scoped, cleanup) = (out.clone(), out.clone(), cleanup.clone());
        col()
            .size_full()
            .child(el().id("root").size_12().on_press(move || root.borrow_mut().push(use_context::<u32>().unwrap())))
            .child(show(
                move || visible.get(),
                move || {
                    let (out, cleanup) = (scoped.clone(), cleanup.clone());
                    scope("provider", move || {
                        assert_eq!(use_context::<u32>(), Some(1));
                        provide(2u32);
                        let (parent, parent_cleanup) = (out.clone(), cleanup.clone());
                        on_cleanup(move || parent_cleanup.borrow_mut().push(use_context::<u32>().unwrap()));
                        col()
                            .child(el().id("parent").size_12().on_press(move || parent.borrow_mut().push(use_context::<u32>().unwrap())))
                            .child(scope("child", move || {
                                assert_eq!(use_context::<u32>(), Some(2));
                                provide(3u32);
                                on_cleanup(move || cleanup.borrow_mut().push(use_context::<u32>().unwrap()));
                                el().id("child").size_12().on_press(move || out.borrow_mut().push(use_context::<u32>().unwrap()))
                            }))
                    })
                },
            ))
            .child(el().id("hide").size_12().on_press(move || visible.set(false)))
    });
    for id in ["child", "parent", "root"] {
        window.tap(id).unwrap();
    }
    assert_eq!(&*seen.borrow(), &[3, 2, 1]);
    window.tap("hide").unwrap();
    assert!(!window.kernel().exists("parent"));
    assert!(!window.kernel().exists("child"));
    assert_eq!(&*cleaned.borrow(), &[3, 2], "children clean up before their provider");
    window.tap("root").unwrap();
    assert_eq!(&*seen.borrow(), &[3, 2, 1, 1]);
    clean(&window);
    window.app().shutdown();
    assert_eq!(&*cleaned.borrow(), &[3, 2], "retired scopes clean up once");
}

#[test]
fn retiring_from_branch_cleanup_skips_the_replacement_factory() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let key = app.run(|| state("branch", || false));
    let baseline = app.work().reactive_nodes;
    let events = Rc::new(RefCell::new(Vec::new()));
    let mounted = events.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            let mounted = mounted.clone();
            keyed(
                move || key.get(),
                move |replacement| {
                    if *replacement {
                        mounted.borrow_mut().push("replacement factory");
                    } else {
                        let cleaned = mounted.clone();
                        on_cleanup(move || {
                            cleaned.borrow_mut().push("cleanup");
                            close_window(current_window());
                        });
                    }
                    text("Branch").id("branch")
                },
            )
        })
        .unwrap();

    assert!(window.kernel().exists("branch"));
    app.run(|| key.set(true));
    assert_eq!(*events.borrow(), ["cleanup"]);
    assert!(window.is_closed());
    assert!(!app.is_failed());
    assert_eq!(app.work().reactive_nodes, baseline);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn explicit_keys_keep_state_across_reordered_reload_and_ui_keeps_deferred_provenance() {
    let reversed = Rc::new(Cell::new(false));
    let order = reversed.clone();
    let mut window = Window::headless(move || {
        let ids = if order.get() { ["right", "left"] } else { ["left", "right"] };
        col().id("counters").size_full().children(ids.map(|id| owned_counter(id, id, 0)))
    });
    window.tap("left").unwrap();
    window.tap("right").unwrap();
    window.tap("right").unwrap();
    let before = window.inspect().find("left").unwrap().target.clone();
    reversed.set(true);
    window.app().hot_reload();
    let snapshot = window.inspect();
    let ids: Vec<_> = snapshot.find("counters").unwrap().children.iter().map(|node| node.id.as_deref()).collect();
    assert_eq!(ids, [Some("right"), Some("left")]);
    assert_eq!(window.kernel().text_of("left-value").as_deref(), Some("1"));
    assert_eq!(window.kernel().text_of("right-value").as_deref(), Some("2"));
    let left = snapshot.find("left").unwrap();
    assert_ne!(left.target, before, "reload mounts a fresh tree while keeping keyed state");
    let made = left.component.as_ref().expect("deferred content retains its #[ui] provenance");
    assert_eq!((made.name.as_str(), made.module.as_str()), ("owned_counter", "integration::scopes"));
    assert!(made.called_at.contains("scopes.rs:"), "{}", made.called_at);
    clean(&window);
}

#[test]
fn scope_keys_with_path_syntax_and_percent_escapes_have_distinct_state() {
    let mut window = Window::headless(|| {
        col()
            .size_full()
            .child(scope("a", || owned_counter("b", "nested", 10)))
            .child(owned_counter("a]/scope[b", "raw", 20))
            .child(owned_counter("a/b", "slash", 30))
            .child(owned_counter("a%2Fb", "percent", 40))
    });
    for (id, expected) in [("nested", "10"), ("raw", "20"), ("slash", "30"), ("percent", "40")] {
        assert_eq!(window.kernel().text_of(&format!("{id}-value")).as_deref(), Some(expected));
    }
    window.tap("raw").unwrap();
    window.tap("slash").unwrap();
    window.app().hot_reload();
    for (id, expected) in [("nested", "10"), ("raw", "21"), ("slash", "31"), ("percent", "40")] {
        assert_eq!(window.kernel().text_of(&format!("{id}-value")).as_deref(), Some(expected));
    }
    clean(&window);
}

struct ReloadValue {
    id: usize,
    drops: Rc<RefCell<Vec<usize>>>,
    panic: bool,
}

impl Drop for ReloadValue {
    fn drop(&mut self) {
        self.drops.borrow_mut().push(self.id);
        assert!(!self.panic, "reload value destructor");
    }
}

#[test]
fn reload_retires_absent_paths_without_touching_other_windows_or_app_state() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let drops = Rc::new(RefCell::new(Vec::new()));
    app.run(|| {
        state("app-value", || ReloadValue { id: 100, drops: drops.clone(), panic: false });
    });
    let key = Rc::new(Cell::new(0usize));
    let (source, retired) = (key.clone(), drops.clone());
    let first = app
        .open_window(WindowOptions::new("Changing"), move || {
            let id = source.get();
            let retired = retired.clone();
            scope(id, move || {
                state("payload", || ReloadValue { id, drops: retired, panic: false });
                text("changing")
            })
        })
        .unwrap();
    let retired = drops.clone();
    let second = app
        .open_window(WindowOptions::new("Stable"), move || {
            let retired = retired.clone();
            scope(0usize, move || {
                state("payload", || ReloadValue { id: 99, drops: retired, panic: false });
                text("stable")
            })
        })
        .unwrap();
    let baseline = app.work().reactive_nodes;
    for id in 1..=6 {
        key.set(id);
        app.hot_reload();
        assert_eq!(app.work().reactive_nodes, baseline, "absent paths cannot accumulate");
        assert_eq!(*drops.borrow(), (0..id).collect::<Vec<_>>());
    }
    app.close_window(first.id());
    assert_eq!(*drops.borrow(), (0..=6).collect::<Vec<_>>());
    assert!(!second.is_closed());
    app.close_window(second.id());
    assert_eq!(drops.borrow().last(), Some(&99));
    assert_eq!(app.work().reactive_nodes, 1, "only App-owned state remains");
    app.shutdown();
    assert_eq!(drops.borrow().last(), Some(&100));
    assert_eq!(app.work().reactive_nodes, 0);
}

#[test]
fn reload_adopts_delayed_handler_state_even_when_it_is_not_looked_up_before_close() {
    let drops = Rc::new(RefCell::new(Vec::new()));
    let retired = drops.clone();
    let mut window = Window::headless(move || {
        let retired = retired.clone();
        scope("stable", move || {
            el().id("pick").role(Role::Button).label("Pick").size_12().on_press(move || {
                let local = state("late", || ReloadValue { id: 10, drops: retired.clone(), panic: false });
                local.update(|v| v.id += 1);
            })
        })
    });
    window.tap("pick").unwrap();
    let baseline = window.app().work().reactive_nodes;
    window.app().hot_reload();
    assert_eq!(window.app().work().reactive_nodes, baseline);
    assert!(drops.borrow().is_empty());
    window.tap("pick").unwrap();
    window.app().hot_reload();
    // No state() call in this incarnation: the surviving owner still owns the retained value.
    window.app().close_window(window.id());
    assert_eq!(*drops.borrow(), [12]);
    assert_eq!(window.app().work().reactive_nodes, 0);
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    window.app().shutdown();
}

#[test]
fn failed_reload_or_retirement_keeps_remaining_state_reachable_for_shutdown() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    for fail_build in [false, true] {
        let key = Rc::new(Cell::new(0usize));
        let drops = Rc::new(RefCell::new(Vec::new()));
        let (source, retired) = (key.clone(), drops.clone());
        let window = Window::headless(move || {
            let id = source.get();
            assert!(!(fail_build && id == 1), "reload build failed");
            let retired = retired.clone();
            scope(id, move || {
                state("payload", || ReloadValue { id, drops: retired, panic: !fail_build && id == 0 });
                text("value")
            })
        });
        key.set(1);
        assert!(catch_unwind(AssertUnwindSafe(|| window.app().hot_reload())).is_err());
        window.app().shutdown();
        window.app().shutdown();
        let mut actual = drops.borrow().clone();
        actual.sort_unstable();
        assert_eq!(actual, if fail_build { vec![0] } else { vec![0, 1] }, "each started destructor is consumed once");
        assert_eq!(window.app().work().reactive_nodes, 0);
    }
}

#[test]
fn retirement_cannot_reuse_a_pending_orphan_during_destructor_reentry() {
    struct Reenter {
        id: usize,
        serial: usize,
        value: usize,
        source: Rc<RefCell<Vec<usize>>>,
        reentered: Rc<Cell<bool>>,
        drops: Rc<RefCell<Vec<usize>>>,
    }

    impl Drop for Reenter {
        fn drop(&mut self) {
            self.drops.borrow_mut().push(self.serial);
            if self.serial < 2 && !self.reentered.replace(true) {
                // Whichever old path retires first rebuilds the other, so map order does
                // not matter. Both old records must already be unavailable for reuse.
                *self.source.borrow_mut() = vec![1 - self.id];
                App::current().hot_reload();
            }
        }
    }

    let source = Rc::new(RefCell::new(vec![0usize, 1]));
    let initialized = Rc::new(Cell::new(0usize));
    let reentered = Rc::new(Cell::new(false));
    let drops = Rc::new(RefCell::new(Vec::new()));
    let (keys, created, entered, retired) = (source.clone(), initialized.clone(), reentered.clone(), drops.clone());
    let mut window = Window::headless(move || {
        let mut root = col().size_full();
        let ids = keys.borrow().clone();
        for id in ids {
            let (source, initialized, reentered, drops) = (keys.clone(), created.clone(), entered.clone(), retired.clone());
            root = root.child(scope(id, move || {
                let value = state("payload", || {
                    let serial = initialized.get();
                    initialized.set(serial + 1);
                    Reenter { id, serial, value: 0, source, reentered, drops }
                });
                col().child(text(move || value.with(|v| format!("{}:{}", v.serial, v.value))).id(format!("value-{id}"))).child(
                    el().id(format!("change-{id}"))
                        .role(Role::Button)
                        .label("Change")
                        .size_12()
                        .on_press(move || value.update(|v| v.value += 1)),
                )
            }));
        }
        root
    });
    source.borrow_mut().clear();
    window.app().hot_reload();
    assert!(reentered.get());
    assert_eq!(initialized.get(), 3, "a retired path starts fresh even during a destructor");
    let survivor = source.borrow()[0];
    let value_id = format!("value-{survivor}");
    assert_eq!(window.kernel().text_of(&value_id).as_deref(), Some("2:0"));
    window.tap(&format!("change-{survivor}")).unwrap();
    assert_eq!(window.kernel().text_of(&value_id).as_deref(), Some("2:1"));
    clean(&window);
    window.app().close_window(window.id());
    let mut actual = drops.borrow().clone();
    actual.sort_unstable();
    assert_eq!(actual, [0, 1, 2]);
    assert_eq!(window.app().work().reactive_nodes, 0);
    window.app().shutdown();
}

#[test]
fn shutdown_releases_all_pending_orphans_after_the_first_destructor_panics() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    struct PanicOnce {
        id: usize,
        panic_next: Rc<Cell<bool>>,
        drops: Rc<RefCell<Vec<usize>>>,
    }

    impl Drop for PanicOnce {
        fn drop(&mut self) {
            self.drops.borrow_mut().push(self.id);
            assert!(!self.panic_next.replace(false), "first orphan destructor");
        }
    }

    let changed = Rc::new(Cell::new(false));
    let panic_next = Rc::new(Cell::new(false));
    let drops = Rc::new(RefCell::new(Vec::new()));
    let (source, fail, retired) = (changed.clone(), panic_next.clone(), drops.clone());
    let window = Window::headless(move || {
        let mut root = col().size_full();
        let ids = if source.get() { 3..4 } else { 0..3 };
        for id in ids {
            let (panic_next, drops) = (fail.clone(), retired.clone());
            root = root.child(scope(id, move || {
                state("payload", || PanicOnce { id, panic_next, drops });
                text("value")
            }));
        }
        root
    });
    changed.set(true);
    panic_next.set(true);
    assert!(catch_unwind(AssertUnwindSafe(|| window.app().hot_reload())).is_err());
    assert_eq!(drops.borrow().len(), 1, "retirement stops after its first started destructor");
    window.app().shutdown();
    window.app().shutdown();
    let mut actual = drops.borrow().clone();
    actual.sort_unstable();
    assert_eq!(actual, [0, 1, 2, 3], "unstarted orphan destructors remain owned by the runtime");
    assert_eq!(window.app().work().reactive_nodes, 0);
}
