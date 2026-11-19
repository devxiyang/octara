//! UI actions obey the caller's execution permissions, including inside `untrack`.

use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use octara::reactive::untrack;
use octara::task::WorkKind;
use octara::view::settle_motion;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Caller {
    Memo,
    WatchSource,
    Property,
    Handler,
    Task,
}

fn rejected_actions() {
    focus("other");
    invoke_after_update("record");
    assert!(invoke("record").is_err());
    assert!(!scroll_to("list", 0.0, 100.0));
    scroll_into_view("row-19");
    assert!(!scroll_to_end("list"));
    settle_motion();
    assert_eq!(instant(|| 17), 17, "instant still evaluates its closure with the caller's permissions");
    assert!(bind_keys([("mod+shift+j", "record", None)]).is_err());
}

fn accepted_actions(lit: Signal<bool>) {
    focus("other");
    assert!(scroll_to("list", 0.0, 100.0));
    assert!(scroll_to_end("list"));
    scroll_into_view("row-19");
    assert!(bind_keys([("mod+shift+j", "record", None)]).is_ok());
    assert!(invoke("record").is_ok());
    invoke_after_update("record");
    settle_motion();
    instant(|| lit.set(false));
}

fn scene(caller: Caller, sampled: bool) -> View {
    let armed = state("armed", || false);
    let lit = state("lit", || false);
    let calls = state("calls", || 0u32);
    on_command("record", move || calls.update(|calls| *calls += 1));
    on_command("animate", move || lit.set(true));
    on_command("act", move || match caller {
        Caller::Handler => accepted_actions(lit),
        Caller::Task => {
            spawn(async move { accepted_actions(lit) });
        }
        _ => armed.set(true),
    });
    bind_keys([("mod+shift+k", "record", None)]).expect("construction may register key bindings");
    let source = move || {
        if armed.get() {
            if sampled {
                untrack(rejected_actions);
            } else {
                rejected_actions();
            }
        }
        "Probe".to_owned()
    };
    let probe = match caller {
        Caller::Memo => text(memo(source)),
        Caller::WatchSource => {
            watch(source, |_, _| {});
            text("Probe")
        }
        Caller::Property => text(source),
        _ => text("Probe"),
    };
    col()
        .size_full()
        .child(probe)
        .child(input().id("editor").label("Editor").w(160))
        .child(input().id("other").label("Other editor").w(160))
        .child(
            col()
                .id("list")
                .w(160)
                .h(80)
                .flex_shrink_0()
                .overflow_y_scroll()
                .children((0..20).map(|index| el().id(format!("row-{index}")).h(20).flex_shrink_0())),
        )
        .child(el().id("moving").size_8().motion(Spring::SNAP).bg(move || rgb(if lit.get() { 0xff0000 } else { 0 })))
        .child(text(move || calls.get().to_string()).id("calls"))
        .into()
}

fn red(window: &Window) -> f32 {
    let node = window.kernel().find("moving").unwrap();
    window.display_list().quads().find(|quad| quad.source == node).and_then(|quad| quad.fill).unwrap().components[0]
}

fn scroll(window: &Window) -> f32 {
    let kernel = window.kernel();
    kernel.scroll_offset(kernel.find("list").unwrap()).1
}

#[test]
fn pure_sources_cannot_change_interaction_bindings_or_motion_even_when_untracked() {
    for caller in [Caller::Memo, Caller::WatchSource, Caller::Property] {
        for sampled in [false, true] {
            let mut window = Window::headless(move || scene(caller, sampled));
            window.tap("editor").unwrap();
            window.invoke("animate").unwrap();
            window.app().advance(Duration::from_millis(60));
            let before = red(&window);
            assert!(before > 0.0 && before < 1.0);
            window.invoke("act").unwrap();
            assert_eq!(window.inspect().focus.as_deref(), Some("editor"), "{caller:?}, untracked={sampled}");
            assert_eq!(window.kernel().text_of("calls").as_deref(), Some("0"));
            assert_eq!(scroll(&window), 0.0);
            assert!((red(&window) - before).abs() < 1e-4, "a rejected action must not land motion");
            assert!(window.app().pending().iter().any(|work| work.kind == WorkKind::Motion));
            let diagnostics = window.app().diagnostics();
            assert_eq!(diagnostics.len(), 9, "each rejected helper reports its caller: {diagnostics:?}");
            assert!(diagnostics.iter().all(|diagnostic| diagnostic.code.id == "WRITE_IN_DERIVE"), "{diagnostics:?}");
            assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
        }
    }
}

#[test]
fn construction_and_initializers_cannot_enqueue_focus_commands_or_instant_motion() {
    for initializer in [false, true] {
        let window = Window::headless(move || {
            let calls = state("calls", || 0u32);
            on_command("record", move || calls.update(|calls| *calls += 1));
            let request = || {
                focus("other");
                invoke_after_update("record");
                assert_eq!(instant(|| 23), 23);
            };
            if initializer {
                let _ = state("request", request);
            } else {
                request();
            }
            col()
                .size_full()
                .child(input().id("editor").label("Editor").w(160))
                .child(input().id("other").label("Other editor").w(160))
                .child(text(move || calls.get().to_string()).id("calls"))
        });
        assert_eq!(window.inspect().focus, None);
        assert_eq!(window.kernel().text_of("calls").as_deref(), Some("0"));
        let diagnostics = window.app().diagnostics();
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics.iter().all(|diagnostic| diagnostic.code.id == "WRITE_IN_BUILD"), "{diagnostics:?}");
        assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    }
}

#[test]
fn handlers_and_tasks_can_focus_invoke_scroll_bind_and_control_motion() {
    for caller in [Caller::Handler, Caller::Task] {
        let mut window = Window::headless(move || scene(caller, false));
        window.invoke("animate").unwrap();
        window.app().advance(Duration::from_millis(60));
        window.invoke("act").unwrap();
        assert_eq!(window.inspect().focus.as_deref(), Some("other"));
        assert_eq!(window.kernel().text_of("calls").as_deref(), Some("2"));
        assert!(scroll(&window) > 0.0);
        assert_eq!(red(&window), 0.0, "an admitted instant action lands its state change");
        assert!(!window.app().pending().iter().any(|work| work.kind == WorkKind::Motion));
        window.press_key("mod+shift+j").unwrap();
        assert_eq!(window.kernel().text_of("calls").as_deref(), Some("3"));
        assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
        assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    }
}

#[test]
fn captured_windows_cannot_dispatch_from_pure_computations_in_either_app() {
    for cross_app in [false, true] {
        let target_app = App::new(HeadlessPlatform::default());
        let captured = Rc::new(RefCell::new(None::<Window>));
        if cross_app {
            let target = target_app
                .open_window(WindowOptions::new("Target"), || {
                    let calls = state("calls", || 0u32);
                    on_command("record", move || calls.update(|calls| *calls += 1));
                    col().size_full().child(text(move || calls.get().to_string()).id("calls"))
                })
                .unwrap();
            *captured.borrow_mut() = Some(target);
        }
        let source_app = if cross_app { App::new(HeadlessPlatform::default()) } else { target_app.clone() };
        let handle = captured.clone();
        let mut source = source_app
            .open_window(WindowOptions::new("Source"), move || {
                let armed = state("armed", || false);
                let calls = state("calls", || 0u32);
                on_command("act", move || armed.set(true));
                on_command("record", move || calls.update(|calls| *calls += 1));
                let handle = handle.clone();
                let probe = memo(move || {
                    if armed.get() {
                        assert!(handle.borrow_mut().as_mut().unwrap().invoke("record").is_err());
                    }
                    "Probe".to_owned()
                });
                col().size_full().child(text(probe)).child(text(move || calls.get().to_string()).id("calls"))
            })
            .unwrap();
        if !cross_app {
            *captured.borrow_mut() = Some(source.clone());
        }
        source.invoke("act").unwrap();
        let mut handle = captured.borrow_mut();
        let target = handle.as_mut().unwrap();
        assert_eq!(target.kernel().text_of("calls").as_deref(), Some("0"));
        target.invoke("record").unwrap();
        assert_eq!(target.kernel().text_of("calls").as_deref(), Some("1"), "host invocation remains available");
        drop(handle);
        let diagnostics = source_app.diagnostics();
        assert_eq!(diagnostics.len(), 1, "cross_app={cross_app}: {diagnostics:?}");
        assert_eq!(diagnostics[0].code.id, "WRITE_IN_DERIVE");
        captured.borrow_mut().take();
        source_app.shutdown();
        target_app.shutdown();
    }
}

#[test]
fn captured_apps_cannot_reload_from_build_or_pure_sources_even_across_apps() {
    for cross_app in [false, true] {
        for phase in ["build", "init", "memo", "watch", "property"] {
            for sampled in [false, true] {
                let target_app = App::new(HeadlessPlatform::default());
                let target_window =
                    target_app.open_window(WindowOptions::new("Target"), || col().size_full().child(text("Target").id("target"))).unwrap();
                let before = target_window.inspect().find("target").unwrap().target.clone();
                let source_app = if cross_app { App::new(HeadlessPlatform::default()) } else { target_app.clone() };
                let destination = target_app.clone();
                let attempted = Rc::new(Cell::new(false));
                let request_attempted = attempted.clone();
                let source_window = source_app
                    .open_window(WindowOptions::new("Source"), move || {
                        let target = destination.clone();
                        let attempted = request_attempted.clone();
                        let request = move || {
                            if !attempted.replace(true) {
                                if sampled {
                                    untrack(|| target.hot_reload());
                                } else {
                                    target.hot_reload();
                                }
                            }
                        };
                        let probe = match phase {
                            "build" => {
                                request();
                                text("Probe")
                            }
                            "init" => {
                                state("initial", || {
                                    request();
                                    0
                                });
                                text("Probe")
                            }
                            "memo" => {
                                let probe = memo(move || {
                                    request();
                                    "Probe".to_owned()
                                });
                                text(probe)
                            }
                            "watch" => {
                                watch(
                                    move || {
                                        request();
                                        0
                                    },
                                    |_, _| {},
                                );
                                text("Probe")
                            }
                            "property" => text(move || {
                                request();
                                "Probe".to_owned()
                            }),
                            _ => unreachable!(),
                        };
                        col().size_full().child(probe)
                    })
                    .unwrap();
                assert!(attempted.get());
                assert_eq!(target_window.inspect().find("target").unwrap().target, before, "{phase}, cross={cross_app}, untrack={sampled}");
                let diagnostics = source_app.diagnostics();
                assert_eq!(diagnostics.len(), 1, "{phase}, cross={cross_app}, untrack={sampled}: {diagnostics:?}");
                assert_eq!(diagnostics[0].code.id, if matches!(phase, "build" | "init") { "WRITE_IN_BUILD" } else { "WRITE_IN_DERIVE" });
                assert!(!source_app.is_failed());
                assert!(!target_app.is_failed());
                if cross_app {
                    assert!(target_app.diagnostics().is_empty(), "rejection belongs to the caller App");
                }
                target_app.hot_reload();
                assert_ne!(target_window.inspect().find("target").unwrap().target, before, "host reload remains available");
                assert!(source_window.layout_issues().is_empty(), "{:?}", source_window.layout_issues());
                assert!(target_window.layout_issues().is_empty(), "{:?}", target_window.layout_issues());
                source_app.shutdown();
                target_app.shutdown();
            }
        }
    }
}

#[test]
fn shutdown_refuses_reload_from_cleanup_and_the_host() {
    let app = App::new(HeadlessPlatform::default());
    let builds = Rc::new(Cell::new(0));
    let root_builds = builds.clone();
    let window = app
        .open_window(WindowOptions::new("Target"), move || {
            root_builds.set(root_builds.get() + 1);
            on_cleanup(|| App::current().hot_reload());
            col().size_full().child(text("Target").id("target"))
        })
        .unwrap();
    app.shutdown();
    app.hot_reload();
    assert_eq!(builds.get(), 1);
    assert!(window.is_closed());
    assert!(!app.is_failed());
    let diagnostics = app.diagnostics();
    assert_eq!(diagnostics.len(), 2, "cleanup and host calls both report rejection: {diagnostics:?}");
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.code.id == "WRITE_DISPOSED"), "{diagnostics:?}");
}

#[test]
fn app_owned_pure_computations_reject_command_dispatch_and_keymap_writes() {
    let app = App::new(HeadlessPlatform::default());
    let target = App::new(HeadlessPlatform::default());
    let foreign = target.clone();
    app.run(|| {
        let result = memo(move || {
            assert!(invoke("unavailable").is_err());
            assert!(untrack(|| invoke("unavailable")).is_err());
            let bindings = std::iter::once_with(|| -> (&'static str, &'static str, Option<&'static str>) {
                panic!("an inadmissible batch must not enter its iterator")
            });
            assert!(foreign.bind_keys(bindings).is_err());
            assert!(foreign.set_keymap(foreign.keymap()).is_err());
            7
        });
        assert_eq!(result.get(), 7);
    });
    assert!(!app.is_failed());
    let diagnostics = app.diagnostics();
    assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.code.id == "WRITE_IN_DERIVE"), "{diagnostics:?}");
    assert!(target.keymap().bindings.is_empty());
    target.shutdown();
    app.shutdown();
}
