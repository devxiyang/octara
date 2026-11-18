//! Resources obey the same phase/lifetime rules and invalidate all windows from one owner.
use octara::app::TargetPlatform;
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use octara_text::{MonoEngine, StyleRun, TextEngine, TextLayout, TextStyle};
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

type Callback = Rc<RefCell<Option<Box<dyn Fn()>>>>;
struct Engine {
    registrations: Rc<Cell<u32>>,
    callback: Callback,
}
impl TextEngine for Engine {
    fn layout(&mut self, text: &str, style: &TextStyle, runs: &[StyleRun], previous: Option<&TextLayout>, clean: usize) -> TextLayout {
        let style = TextStyle { font_size: style.font_size * (self.registrations.get() + 1) as f32, ..*style };
        MonoEngine.layout(text, &style, runs, previous, clean)
    }
    fn register_font(&mut self, _: &'static [u8]) {
        self.registrations.set(self.registrations.get() + 1);
        if let Some(callback) = self.callback.borrow().as_ref() {
            callback();
        }
    }
    fn register_fallback_font(&mut self, data: &'static [u8], _: &[[u8; 4]]) {
        self.register_font(data);
    }
}
fn app() -> (App, Rc<Cell<u32>>, Callback) {
    let registrations = Rc::new(Cell::new(0));
    let callback = Rc::new(RefCell::new(None));
    let app = App::with_config(
        HeadlessPlatform::default(),
        AppConfig {
            theme: Theme::light(),
            target_platform: TargetPlatform::MacOs,
            text_engine: Box::new(Engine { registrations: registrations.clone(), callback: callback.clone() }),
        },
    );
    (app, registrations, callback)
}
fn change_all(app: &App) {
    app.set_theme(Theme::dark());
    app.register_font(&[]);
    app.register_fallback_font(&[], &[*b"Hani"]);
    app.set_target_platform(TargetPlatform::Ios);
    app.set_reduced_motion(true);
}
fn label() -> View {
    text("Hello").text_size(px(16.0)).id("label").into()
}
fn width(window: &Window) -> f32 {
    let k = window.kernel();
    k.text_metrics(k.find("label").unwrap()).width
}

#[test]
fn build_init_derive_and_observe_cannot_mutate_resources_even_across_apps() {
    for cross_app in [false, true] {
        for phase in ["build", "init", "derive", "observe"] {
            let (target, registrations, _) = app();
            let source = if cross_app { App::new(HeadlessPlatform::default()) } else { target.clone() };
            let destination = target.clone();
            let window = source
                .open_window(WindowOptions::new(phase), move || {
                    let target = destination.clone();
                    match phase {
                        "build" => change_all(&target),
                        "init" => {
                            state("initial", || {
                                change_all(&target);
                                0
                            });
                        }
                        "derive" => {
                            let value = memo(move || {
                                change_all(&target);
                                0
                            });
                            let _ = untrack(|| value.get());
                        }
                        "observe" => {
                            watch(
                                move || {
                                    change_all(&target);
                                    0
                                },
                                |_, _| {},
                            );
                        }
                        _ => unreachable!(),
                    }
                    label()
                })
                .unwrap();
            assert_eq!(target.theme().name, "light", "{phase}, cross={cross_app}");
            assert_eq!(target.target_platform(), TargetPlatform::MacOs);
            assert_eq!(registrations.get(), 0);
            let check = target.open_window(WindowOptions::new("Check"), label).unwrap();
            assert!(!check.kernel().reduced_motion());
            assert_eq!(width(&check), 40.0);
            assert!(
                source
                    .diagnostics()
                    .iter()
                    .any(|d| d.code.id == if matches!(phase, "build" | "init") { "WRITE_IN_BUILD" } else { "WRITE_IN_DERIVE" })
            );
            assert!(!source.is_failed());
            source.close_window(window.id());
            source.shutdown();
            target.shutdown();
        }
    }
}

#[test]
fn theme_convenience_function_uses_the_same_gate() {
    let window = Window::headless(|| {
        set_theme(Theme::dark());
        label()
    });
    assert_eq!(window.app().theme().name, "light");
    assert!(window.app().diagnostics().iter().any(|d| d.code.id == "WRITE_IN_BUILD"));
    window.app().shutdown();
}

#[test]
fn font_changes_remeasure_every_window_without_rebuilding_views() {
    let (app, registrations, _) = app();
    let a = app.open_window(WindowOptions::new("A"), label).unwrap();
    let b = app.open_window(WindowOptions::new("B"), label).unwrap();
    let targets = (a.inspect().find("label").unwrap().target.clone(), b.inspect().find("label").unwrap().target.clone());
    app.register_font(&[]);
    assert_eq!((width(&a), width(&b)), (80.0, 80.0));
    app.register_fallback_font(&[], &[*b"Hani"]);
    assert_eq!((width(&a), width(&b)), (120.0, 120.0));
    assert_eq!(registrations.get(), 2);
    assert_eq!(targets, (a.inspect().find("label").unwrap().target.clone(), b.inspect().find("label").unwrap().target.clone()));
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn settings_before_any_window_are_preserved_and_theme_watch_can_update_resources() {
    let (app, _, _) = app();
    app.set_reduced_motion(true);
    app.set_target_platform(TargetPlatform::Ios);
    let toggle = app.run(|| {
        let toggle = state("dark", || false);
        watch(move || toggle.get(), |dark, _| set_theme(if *dark { Theme::dark() } else { Theme::light() }));
        toggle
    });
    let a = app.open_window(WindowOptions::new("A"), label).unwrap();
    let b = app.open_window(WindowOptions::new("B"), label).unwrap();
    app.run(|| toggle.set(true));
    for window in [&a, &b] {
        assert!(window.kernel().reduced_motion());
        assert!(window.kernel().touch());
        assert_eq!(window.kernel().visual_of("label").unwrap().text, Theme::dark().get(Token::Text));
    }
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn closed_and_failed_apps_cannot_mutate_resources() {
    let (closed, registrations, _) = app();
    closed.shutdown();
    change_all(&closed);
    assert_eq!(closed.theme().name, "light");
    assert_eq!(closed.target_platform(), TargetPlatform::MacOs);
    assert_eq!(registrations.get(), 0);
    assert!(closed.diagnostics().iter().any(|d| d.code.id == "WRITE_DISPOSED"));

    let (failed, registrations, _) = app();
    assert!(catch_unwind(AssertUnwindSafe(|| failed.run(|| panic!("failed app")))).is_err());
    for operation in 0..5 {
        assert!(
            catch_unwind(AssertUnwindSafe(|| match operation {
                0 => failed.set_theme(Theme::dark()),
                1 => failed.register_font(&[]),
                2 => failed.register_fallback_font(&[], &[*b"Hani"]),
                3 => failed.set_target_platform(TargetPlatform::Ios),
                _ => failed.set_reduced_motion(true),
            }))
            .is_err()
        );
    }
    assert_eq!(failed.theme().name, "light");
    assert_eq!(registrations.get(), 0);
    failed.shutdown();
}

#[test]
fn provider_reads_do_not_subscribe_the_caller_and_provider_writes_are_rejected() {
    let (app, registrations, callback) = app();
    let (source, font_request) = app.run(|| (state("source", || 0), state("fonts", || 0)));
    *callback.borrow_mut() = Some(Box::new(move || {
        let _ = source.get();
        change_all(&App::current());
        App::current().shutdown();
    }));
    app.run(|| {
        watch(move || font_request.get(), |_, _| App::current().register_font(&[]));
    });
    assert_eq!(registrations.get(), 1, "nested font registration was rejected before touching the engine");
    app.run(|| source.set(1));
    assert_eq!(registrations.get(), 1, "provider reads did not enroll the watch");
    app.run(|| font_request.set(1));
    assert_eq!(registrations.get(), 2);
    assert_eq!(app.theme().name, "light");
    assert!(!app.is_shutdown());
    assert!(app.diagnostics().iter().all(|d| d.code.id == "WRITE_IN_DERIVE"), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn native_text_queries_decline_during_font_registration_and_resume_afterwards() {
    let (app, _, callback) = app();
    let mut window =
        app.open_window(WindowOptions::new("Input"), || text_input().id("field").label("Text").value("Hello".to_owned())).unwrap();
    window.dispatch(Event::Focus("field".into())).unwrap();
    let layout = {
        let host = window.platform_window();
        host.as_any().downcast_ref::<octara::platform::HeadlessWindow>().unwrap().text_input.as_ref().unwrap().layout.clone().unwrap()
    };
    let query = layout.clone();
    let called = Rc::new(Cell::new(false));
    let seen = called.clone();
    *callback.borrow_mut() = Some(Box::new(move || {
        seen.set(true);
        let document = octara::kernel::TextDocument::new("Hello".into(), 5, 5);
        assert!(query.layout(&document).is_none());
    }));
    app.register_font(&[]);
    assert!(called.get());
    assert!(layout.layout(&octara::kernel::TextDocument::new("Hello".into(), 5, 5)).is_some());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn provider_panic_fails_the_app_without_publishing_a_partial_frame() {
    let (app, _, callback) = app();
    let window = app.open_window(WindowOptions::new("A"), label).unwrap();
    let frames = window.frames_presented();
    *callback.borrow_mut() = Some(Box::new(|| panic!("font provider")));
    assert!(catch_unwind(AssertUnwindSafe(|| app.register_font(&[]))).is_err());
    assert!(app.is_failed());
    assert_eq!(window.frames_presented(), frames);
    app.shutdown();
    assert!(window.is_closed());
}

#[test]
fn window_cleanup_can_update_a_live_app_but_app_shutdown_revokes_resource_writes() {
    let (app, registrations, _) = app();
    let surviving = app.open_window(WindowOptions::new("Surviving"), label).unwrap();
    let retiring = app
        .open_window(WindowOptions::new("Retiring"), || {
            on_cleanup(|| change_all(&App::current()));
            label()
        })
        .unwrap();
    app.close_window(retiring.id());
    assert_eq!(registrations.get(), 2);
    assert_eq!(width(&surviving), 120.0);
    assert!(surviving.kernel().touch());
    assert!(surviving.kernel().reduced_motion());
    assert_eq!(surviving.kernel().theme().name, "dark");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.run(|| {
        on_cleanup(|| {
            App::current().set_theme(Theme::light());
            App::current().register_font(&[]);
        })
    });
    app.shutdown();
    assert_eq!(app.theme().name, "dark");
    assert_eq!(registrations.get(), 2);
    assert!(app.diagnostics().iter().all(|d| d.code.id == "WRITE_DISPOSED"), "{:?}", app.diagnostics());
}
