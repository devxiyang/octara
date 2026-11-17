//! UI actions obey the caller's execution permissions, including inside `untrack`.

use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use octara::task::WorkKind;
use std::cell::RefCell;
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
        .child(text_input().id("editor").label("Editor").w(160))
        .child(text_input().id("other").label("Other editor").w(160))
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
                .child(text_input().id("editor").label("Editor").w(160))
                .child(text_input().id("other").label("Other editor").w(160))
                .child(text(move || calls.get().to_string()).id("calls"))
                .into()
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
                    col().size_full().child(text(move || calls.get().to_string()).id("calls")).into()
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
                col().size_full().child(text(probe)).child(text(move || calls.get().to_string()).id("calls")).into()
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
fn app_owned_pure_computations_reject_commands_without_requiring_a_window() {
    let app = App::new(HeadlessPlatform::default());
    app.run(|| {
        let result = memo(|| {
            assert!(invoke("unavailable").is_err());
            assert!(untrack(|| invoke("unavailable")).is_err());
            7
        });
        assert_eq!(result.get(), 7);
    });
    assert!(!app.is_failed());
    let diagnostics = app.diagnostics();
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.code.id == "WRITE_IN_DERIVE"), "{diagnostics:?}");
    app.shutdown();
}
